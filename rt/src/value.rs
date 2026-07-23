//! Plix value semantics — shared by the tree-walking interpreter and by
//! Cranelift-compiled native code (all `plix_*` extern functions).
//!
//! Type rules:
//!   - arithmetic: int op int -> int (overflow promotes to float);
//!     mixed numeric -> float; string + string concatenates;
//!     array + array concatenates.
//!   - "/" is true division and always yields float; use idiv() for ints.
//!   - "==" is structural (deep) equality for arrays/objects.
//!   - truthy: false, null, 0, 0.0, "", [], {} are falsy; everything else true.

use crate::heap::*;

pub type OpResult = Result<V, String>;

#[inline]
fn type_err(op: &str, a: V, b: V) -> String {
    format!(
        "unsupported operand types for '{}': {} and {}",
        op,
        kind_name(a),
        kind_name(b)
    )
}
#[inline]
fn type_err1(op: &str, a: V) -> String {
    format!("unsupported operand type for '{}': {}", op, kind_name(a))
}

// ---------------------------------------------------------------------------
// truthiness / display
// ---------------------------------------------------------------------------

pub fn truthy(v: V) -> bool {
    if is_null(v) || v == FALSE {
        return false;
    }
    if v == TRUE {
        return true;
    }
    if is_int(v) {
        return as_int(v) != 0;
    }
    unsafe {
        match payload(v) {
            HeapObj::Float(f) => *f != 0.0,
            HeapObj::Str(s) => !s.is_empty(),
            HeapObj::Array(a) => !a.is_empty(),
            HeapObj::Map(m) => !m.is_empty(),
            _ => true, // structs, instances, bound methods: always truthy
        }
    }
}

/// str(v): user-facing conversion (used by str(), say(), interpolation).
pub fn to_display(v: V) -> String {
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
    unsafe {
        match payload(v) {
            HeapObj::Float(f) => format!("{:?}", f),
            HeapObj::Str(s) => s.clone(),
            _ => to_repr_depth(v, 0),
        }
    }
}

/// repr(v): debug form; strings quoted, containers recursive.
pub fn to_repr(v: V) -> String {
    to_repr_depth(v, 0)
}

fn to_repr_depth(v: V, depth: usize) -> String {
    if depth > 16 {
        return "...".to_string();
    }
    if is_null(v) {
        return "null".into();
    }
    if v == TRUE {
        return "true".into();
    }
    if v == FALSE {
        return "false".into();
    }
    if is_int(v) {
        return format!("{}", as_int(v));
    }
    unsafe {
        match payload(v) {
            HeapObj::Float(f) => format!("{:?}", f),
            HeapObj::Str(s) => format!("\"{}\"", escape_str(s)),
            HeapObj::Array(items) => {
                let parts: Vec<String> =
                    items.iter().map(|&x| to_repr_depth(x, depth + 1)).collect();
                format!("[{}]", parts.join(", "))
            }
            HeapObj::Map(m) => {
                let mut parts: Vec<String> = m
                    .iter()
                    .map(|(k, &x)| format!("{}: {}", k, to_repr_depth(x, depth + 1)))
                    .collect();
                parts.sort();
                format!("{{{}}}", parts.join(", "))
            }
            HeapObj::Cell(x) => format!("<cell {}>", to_repr_depth(*x, depth + 1)),
            HeapObj::ClsNative { name, .. } => format!("<func {}>", name),
            HeapObj::ClsAst { .. } => "<func>".to_string(),
            HeapObj::Builtin(id) => format!("<builtin {}>", crate::builtins::builtin_name(*id)),
            HeapObj::PyObj(p) | HeapObj::PyBound(p, _) => crate::pyffi::py_repr_handle(*p),
            HeapObj::StructDef(info) => format!("<struct {}>", info.name),
            HeapObj::Instance { def, fields } => {
                let name = struct_name_of(v).unwrap_or_default();
                let fnames: Vec<String> = struct_info(*def)
                    .map(|info| info.fields.iter().map(|f| f.name.clone()).collect())
                    .unwrap_or_default();
                let mut parts: Vec<String> = Vec::with_capacity(fields.len());
                for (i, &fv) in fields.iter().enumerate() {
                    let fname = fnames.get(i).cloned().unwrap_or_else(|| format!("_{}", i));
                    parts.push(format!("{}: {}", fname, to_repr_depth(fv, depth + 1)));
                }
                format!("{} {{ {} }}", name, parts.join(", "))
            }
            HeapObj::Bound { f, .. } => {
                let _ = f;
                "<bound method>".to_string()
            }
            HeapObj::Buffer(_) => "<buffer>".to_string(),
            HeapObj::ForeignLib(_) => "<foreign_lib>".to_string(),
        }
    }
}

pub fn escape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// equality / ordering
// ---------------------------------------------------------------------------

pub fn values_eq(a: V, b: V) -> bool {
    eq_depth(a, b, 0)
}

fn eq_depth(a: V, b: V, depth: usize) -> bool {
    if a == b {
        return true; // same bits: same int/bool/null/object identity
    }
    if depth > 64 {
        return true; // cycle guard
    }
    if (is_int(a) && matches!(unsafe { payload_opt(b) }, Some(HeapObj::Float(_))))
        || (is_int(b) && matches!(unsafe { payload_opt(a) }, Some(HeapObj::Float(_))))
    {
        return as_float(a) == as_float(b);
    }
    unsafe {
        match (payload_opt(a), payload_opt(b)) {
            (Some(HeapObj::Float(x)), Some(HeapObj::Float(y))) => x == y,
            (Some(HeapObj::Str(x)), Some(HeapObj::Str(y))) => x == y,
            (Some(HeapObj::Array(x)), Some(HeapObj::Array(y))) => {
                x.len() == y.len()
                    && x.iter()
                        .zip(y.iter())
                        .all(|(&p, &q)| eq_depth(p, q, depth + 1))
            }
            (Some(HeapObj::Map(x)), Some(HeapObj::Map(y))) => {
                x.len() == y.len()
                    && x.iter()
                        .all(|(k, &v)| y.get(k).is_some_and(|&w| eq_depth(v, w, depth + 1)))
            }
            (
                Some(HeapObj::Instance {
                    def: d1,
                    fields: f1,
                }),
                Some(HeapObj::Instance {
                    def: d2,
                    fields: f2,
                }),
            ) => {
                // structural equality within the same struct type
                d1 == d2
                    && f1.len() == f2.len()
                    && f1
                        .iter()
                        .zip(f2.iter())
                        .all(|(&p, &q)| eq_depth(p, q, depth + 1))
            }
            _ => false,
        }
    }
}

#[allow(static_mut_refs)]
unsafe fn payload_opt(v: V) -> Option<&'static HeapObj> {
    if is_ptr(v) {
        Some(payload(v))
    } else {
        None
    }
}

/// < <= > >= : numbers and strings only.
pub fn compare(a: V, b: V) -> Result<std::cmp::Ordering, String> {
    let num = |v: V| -> Option<f64> {
        if is_int(v) {
            Some(as_int(v) as f64)
        } else {
            unsafe {
                match payload_opt(v) {
                    Some(HeapObj::Float(f)) => Some(*f),
                    _ => None,
                }
            }
        }
    };
    if let (Some(x), Some(y)) = (num(a), num(b)) {
        return x
            .partial_cmp(&y)
            .ok_or_else(|| "comparison with NaN".to_string());
    }
    unsafe {
        if let (Some(HeapObj::Str(x)), Some(HeapObj::Str(y))) = (payload_opt(a), payload_opt(b)) {
            return Ok(x.cmp(y));
        }
    }
    Err(format!(
        "cannot compare {} and {}",
        kind_name(a),
        kind_name(b)
    ))
}

// ---------------------------------------------------------------------------
// arithmetic
// ---------------------------------------------------------------------------

pub fn add(a: V, b: V) -> OpResult {
    if is_int(a) && is_int(b) {
        let (x, y) = (as_int(a), as_int(b));
        return Ok(match x.checked_add(y) {
            Some(r) => mk_int(r),
            None => mk_float_unchecked(x as f64 + y as f64),
        });
    }
    // string concat - optimized with RC==1 reuse and capacity doubling
    unsafe {
        if let (Some(HeapObj::Str(x)), Some(HeapObj::Str(y))) = (payload_opt(a), payload_opt(b)) {
            // fast path: if left string is uniquely owned (rc==1 or 2 including temp), reuse it
            if is_ptr(a) {
                let rc = (* (a as *const crate::heap::HeapBox)).rc.get();
                if rc <= 2 {
                    // try to mutate in place
                    let p = payload_mut(a);
                    if let HeapObj::Str(s) = &mut *p {
                        // reserve extra to make strcat O(n) amortized: grow 1.5x
                        let needed = s.len() + y.len();
                        if s.capacity() < needed {
                            let new_cap = (needed * 3 / 2 + 8).next_power_of_two().max(needed);
                            s.reserve(new_cap - s.len());
                        }
                        s.push_str(y);
                        // return same V without new allocation (keep rc)
                        return Ok(a);
                    }
                }
            }
            // fallback: new allocation with spare capacity for future concats
            let cap = (x.len() + y.len()) * 2 + 8;
            let mut s = String::with_capacity(cap);
            s.push_str(x);
            s.push_str(y);
            return Ok(mk_string(s));
        }
        if let (Some(HeapObj::Array(x)), Some(HeapObj::Array(y))) = (payload_opt(a), payload_opt(b))
        {
            // array concat reuse if rc<=2
            if is_ptr(a) {
                let rc = (* (a as *const crate::heap::HeapBox)).rc.get();
                if rc <= 2 {
                    let p = payload_mut(a);
                    if let HeapObj::Array(arr) = &mut *p {
                        arr.reserve(y.len());
                        for &v in y.iter() {
                            retain_locked(v);
                            arr.push(v);
                        }
                        return Ok(a);
                    }
                }
            }
            let mut v = Vec::with_capacity(x.len() + y.len());
            v.extend(x.iter().copied());
            v.extend(y.iter().copied());
            return Ok(mk_array(v));
        }
    }
    if numeric(a) && numeric(b) {
        return Ok(mk_float_unchecked(as_float(a) + as_float(b)));
    }
    Err(type_err("+", a, b))
}

pub fn sub(a: V, b: V) -> OpResult {
    if is_int(a) && is_int(b) {
        let (x, y) = (as_int(a), as_int(b));
        return Ok(match x.checked_sub(y) {
            Some(r) => mk_int(r),
            None => mk_float_unchecked(x as f64 - y as f64),
        });
    }
    if numeric(a) && numeric(b) {
        return Ok(mk_float_unchecked(as_float(a) - as_float(b)));
    }
    Err(type_err("-", a, b))
}

pub fn mul(a: V, b: V) -> OpResult {
    if is_int(a) && is_int(b) {
        let (x, y) = (as_int(a), as_int(b));
        return Ok(match x.checked_mul(y) {
            Some(r) => mk_int(r),
            None => mk_float_unchecked(x as f64 * y as f64),
        });
    }
    if numeric(a) && numeric(b) {
        return Ok(mk_float_unchecked(as_float(a) * as_float(b)));
    }
    // string repetition: "ab" * 3
    unsafe {
        if let (Some(HeapObj::Str(s)), true) = (payload_opt(a), is_int(b)) {
            let n = as_int(b);
            if (0..(1 << 24)).contains(&n) {
                return Ok(mk_string(s.repeat(n as usize)));
            }
        }
    }
    Err(type_err("*", a, b))
}

pub fn div(a: V, b: V) -> OpResult {
    if numeric(a) && numeric(b) {
        let y = as_float(b);
        if y == 0.0 {
            return Err("division by zero".into());
        }
        return Ok(mk_float_unchecked(as_float(a) / y));
    }
    Err(type_err("/", a, b))
}

pub fn int_div(a: V, b: V) -> OpResult {
    if is_int(a) && is_int(b) {
        let (x, y) = (as_int(a), as_int(b));
        if y == 0 {
            return Err("integer division by zero".into());
        }
        return Ok(mk_int(x.wrapping_div(y)));
    }
    Err(type_err("idiv", a, b))
}

pub fn rem(a: V, b: V) -> OpResult {
    if is_int(a) && is_int(b) {
        let (x, y) = (as_int(a), as_int(b));
        if y == 0 {
            return Err("remainder by zero".into());
        }
        return Ok(mk_int(x.wrapping_rem(y)));
    }
    if numeric(a) && numeric(b) {
        let y = as_float(b);
        if y == 0.0 {
            return Err("remainder by zero".into());
        }
        return Ok(mk_float_unchecked(as_float(a) % y));
    }
    Err(type_err("%", a, b))
}

pub fn neg(a: V) -> OpResult {
    if is_int(a) {
        let x = as_int(a);
        return Ok(match x.checked_neg() {
            Some(r) => mk_int(r),
            None => mk_float_unchecked(-(x as f64)),
        });
    }
    if numeric(a) {
        return Ok(mk_float_unchecked(-as_float(a)));
    }
    Err(type_err1("unary -", a))
}

#[inline]
fn numeric(v: V) -> bool {
    if is_int(v) {
        return true;
    }
    unsafe { matches!(payload_opt(v), Some(HeapObj::Float(_))) }
}

// bitwise (ints only)
pub fn band(a: V, b: V) -> OpResult {
    two_ints("&", a, b, |x, y| x & y)
}
pub fn bor(a: V, b: V) -> OpResult {
    two_ints("|", a, b, |x, y| x | y)
}
pub fn bxor(a: V, b: V) -> OpResult {
    two_ints("^", a, b, |x, y| x ^ y)
}
pub fn shl(a: V, b: V) -> OpResult {
    if is_int(a) && is_int(b) {
        let (x, y) = (as_int(a), as_int(b));
        if y < 0 {
            return Err("negative shift count".into());
        }
        return Ok(if y >= 62 { mk_int(0) } else { mk_int(x << y) });
    }
    Err(type_err("<<", a, b))
}
pub fn shr(a: V, b: V) -> OpResult {
    if is_int(a) && is_int(b) {
        let (x, y) = (as_int(a), as_int(b));
        if y < 0 {
            return Err("negative shift count".into());
        }
        return Ok(if y >= 62 {
            mk_int(if x >= 0 { 0 } else { -1 })
        } else {
            mk_int(x >> y)
        });
    }
    Err(type_err(">>", a, b))
}
pub fn bitnot(a: V) -> OpResult {
    if is_int(a) {
        return Ok(mk_int(!as_int(a)));
    }
    Err(type_err1("~", a))
}
fn two_ints(op: &str, a: V, b: V, f: impl FnOnce(i64, i64) -> i64) -> OpResult {
    if is_int(a) && is_int(b) {
        return Ok(mk_int(f(as_int(a), as_int(b))));
    }
    Err(type_err(op, a, b))
}

// ---------------------------------------------------------------------------
// length / indexing / member access / slicing
// ---------------------------------------------------------------------------

pub fn length(v: V) -> OpResult {
    unsafe {
        match payload_opt(v) {
            Some(HeapObj::Str(s)) => Ok(mk_int(s.chars().count() as i64)),
            Some(HeapObj::Array(a)) => Ok(mk_int(a.len() as i64)),
            Some(HeapObj::Map(m)) => Ok(mk_int(m.len() as i64)),
            Some(HeapObj::PyObj(p)) => crate::pyffi::py_len(*p),
            Some(HeapObj::PyBound(..)) => Err("cannot take len() of a bound attribute".into()),
            _ => Err(format!("value of type {} has no length", kind_name(v))),
        }
    }
}

fn norm_index(i: i64, len: i64) -> Result<usize, String> {
    let idx = if i < 0 { i + len } else { i };
    if idx < 0 || idx >= len {
        return Err(format!("index {} out of bounds (len {})", i, len));
    }
    Ok(idx as usize)
}

pub fn index_get(v: V, idx: V) -> OpResult {
    unsafe {
        match payload_opt(v) {
            Some(HeapObj::Array(items)) => {
                if !is_int(idx) {
                    return Err(format!("array index must be int, got {}", kind_name(idx)));
                }
                let i = norm_index(as_int(idx), items.len() as i64)?;
                Ok(use_locked(items[i]))
            }
            Some(HeapObj::Str(s)) => {
                if !is_int(idx) {
                    return Err(format!("string index must be int, got {}", kind_name(idx)));
                }
                let i = norm_index(as_int(idx), s.chars().count() as i64)?;
                let ch = s.chars().nth(i).unwrap();
                Ok(mk_str_from(&ch.to_string()))
            }
            Some(HeapObj::Map(m)) => {
                let key = display_key(idx)?;
                match m.get(&key) {
                    Some(&x) => Ok(use_locked(x)),
                    None => Err(format!("object has no key \"{}\"", key)),
                }
            }
            Some(HeapObj::PyObj(p)) => crate::pyffi::py_index_get(*p, idx),
            Some(HeapObj::PyBound(..)) => Err("cannot index a bound attribute".into()),
            _ => Err(format!("cannot index value of type {}", kind_name(v))),
        }
    }
}

pub fn index_set(v: V, idx: V, val: V) -> OpResult {
    unsafe {
        match payload_opt(v) {
            Some(HeapObj::Array(_)) => {
                if !is_int(idx) {
                    return Err(format!("array index must be int, got {}", kind_name(idx)));
                }
                if let HeapObj::Array(items) = &mut *payload_mut(v) {
                    let i = norm_index(as_int(idx), items.len() as i64)?;
                    retain_locked(idx);
                    release_locked(idx);
                    retain_locked(val);
                    let old = std::mem::replace(&mut items[i], val);
                    release_locked(old);
                }
                Ok(val)
            }
            Some(HeapObj::Map(_)) => {
                let key = display_key(idx)?;
                if let HeapObj::Map(m) = &mut *payload_mut(v) {
                    retain_locked(val);
                    if let Some(old) = m.insert(key, val) {
                        release_locked(old);
                    }
                }
                Ok(val)
            }
            Some(HeapObj::PyObj(p)) => crate::pyffi::py_index_set(*p, idx, val),
            Some(HeapObj::PyBound(..)) => Err("cannot index a bound attribute".into()),
            _ => Err(format!(
                "cannot index-assign value of type {}",
                kind_name(v)
            )),
        }
    }
}

fn display_key(idx: V) -> Result<String, String> {
    if is_null(idx) || is_bool(idx) || is_int(idx) {
        return Ok(to_display(idx));
    }
    unsafe {
        match payload_opt(idx) {
            Some(HeapObj::Str(s)) => Ok(s.clone()),
            _ => Err(format!(
                "object key must be a string, got {}",
                kind_name(idx)
            )),
        }
    }
}

pub fn member_get(v: V, name: &str) -> OpResult {
    unsafe {
        match payload_opt(v) {
            Some(HeapObj::Map(m)) => match m.get(name) {
                Some(&x) => Ok(use_locked(x)),
                None => Err(format!("object has no member \"{}\"", name)),
            },
            Some(HeapObj::PyObj(p)) => crate::pyffi::py_getattr(*p, name),
            Some(HeapObj::PyBound(..)) => Err("cannot take member of a bound attribute".into()),
            Some(HeapObj::Array(_)) => builtin_array_member(name),
            Some(HeapObj::Str(_)) => builtin_str_member(name),
            Some(HeapObj::StructDef(info)) => {
                // associated functions: Point.new, Point.origin ...
                match info.methods.get(name) {
                    Some(&f) => Ok(use_locked(f)),
                    None => Err(format!(
                        "struct {} has no associated item \"{}\"",
                        info.name, name
                    )),
                }
            }
            Some(HeapObj::Instance { def, fields }) => {
                let info = struct_info(*def)
                    .ok_or_else(|| "instance with invalid type descriptor".to_string())?;
                if let Some(&i) = info.index.get(name) {
                    return Ok(use_locked(fields[i]));
                }
                if let Some(&f) = info.methods.get(name) {
                    return Ok(mk_bound(v, f));
                }
                // trait methods: usable when the method name is unambiguous
                let mut hits: Vec<V> = Vec::new();
                for tbl in info.traits.values() {
                    if let Some(&f) = tbl.get(name) {
                        hits.push(f);
                    }
                }
                match hits.len() {
                    0 => Err(format!(
                        "instance of {} has no member \"{}\"",
                        info.name, name
                    )),
                    1 => Ok(mk_bound(v, hits[0])),
                    _ => Err(format!(
                        "method \"{}\" of {} is ambiguous (multiple traits provide it)",
                        name, info.name
                    )),
                }
            }
            Some(HeapObj::Bound { .. }) => Err("cannot take member of a bound method".into()),
            _ => Err(format!(
                "value of type {} has no member \"{}\"",
                kind_name(v),
                name
            )),
        }
    }
}

/// Runtime check: does `v` satisfy the declared field/param type `ty`?
/// "" and "any" accept everything; "float" also accepts int (widening,
/// caller may want to substitute the converted value via `widen_num`).
pub fn value_fits(v: V, ty: &str) -> bool {
    match ty {
        "" | "any" => true,
        "int" => is_int(v),
        "float" => is_int(v) || is_float(v),
        "str" => is_str(v),
        "bool" => is_bool(v),
        "null" => is_null(v),
        "array" => is_kind(v, |o| matches!(o, HeapObj::Array(_))),
        "map" => is_kind(v, |o| matches!(o, HeapObj::Map(_))),
        "func" => is_kind(v, |o| {
            matches!(
                o,
                HeapObj::ClsNative { .. }
                    | HeapObj::ClsAst { .. }
                    | HeapObj::Builtin(_)
                    | HeapObj::Bound { .. }
            )
        }),
        other => unsafe {
            // a struct name: instance of exactly that struct
            matches!(struct_name_of(v), Some(n) if n == other)
        },
    }
}

#[inline]
fn is_kind(v: V, f: impl FnOnce(&HeapObj) -> bool) -> bool {
    unsafe { payload_opt(v).map(f).unwrap_or(false) }
}

#[inline]
fn is_float(v: V) -> bool {
    is_kind(v, |o| matches!(o, HeapObj::Float(_)))
}
#[inline]
fn is_str(v: V) -> bool {
    is_kind(v, |o| matches!(o, HeapObj::Str(_)))
}

/// Build an instance of struct `def` from named field values; fills
/// defaults, validates required/unknown fields and declared field types.
pub fn instantiate(def: V, pairs: Vec<(String, V)>) -> OpResult {
    unsafe {
        let info = match struct_info(def) {
            Some(i) => i,
            None => {
                return Err(format!(
                    "cannot construct an instance of {}",
                    kind_name(def)
                ))
            }
        };
        let mut provided = vec![false; info.fields.len()];
        for (k, _) in &pairs {
            let Some(&i) = info.index.get(k.as_str()) else {
                return Err(format!("struct {} has no field \"{}\"", info.name, k));
            };
            if provided[i] {
                return Err(format!("duplicate field \"{}\" in struct literal", k));
            }
            provided[i] = true;
        }
        let mut fields: Vec<V> = vec![NULL; info.fields.len()];
        for (k, val) in pairs {
            let i = info.index[k.as_str()];
            fields[i] = coerce_field(val, &info.fields[i])?;
        }
        for (i, ft) in info.fields.iter().enumerate() {
            if !provided[i] {
                if ft.has_default {
                    fields[i] = use_locked(ft.default);
                } else {
                    return Err(format!(
                        "struct {}: missing field \"{}\"",
                        info.name, ft.name
                    ));
                }
            }
        }
        Ok(mk_instance(def, fields))
    }
}

fn coerce_field(val: V, ft: &FieldInfo) -> Result<V, String> {
    if value_fits(val, &ft.ty) {
        // widen int -> float when the field is declared float
        if ft.ty == "float" && is_int(val) {
            return Ok(mk_float_unchecked(as_int(val) as f64));
        }
        return Ok(val);
    }
    Err(format!(
        "field \"{}\": expected {}, got {}",
        ft.name,
        if ft.ty.is_empty() { "any" } else { &ft.ty },
        type_name(val)
    ))
}

/// Array pseudo-methods: arr.length  (kept tiny; functions live in builtins).
fn builtin_array_member(name: &str) -> OpResult {
    Err(format!(
        "array has no member \"{}\" (try a function like len, push, map)",
        name
    ))
}
fn builtin_str_member(name: &str) -> OpResult {
    let _ = name;
    Err("strings have no members (try a function like trim, split, upper)".into())
}

pub fn member_set(v: V, name: &str, val: V) -> OpResult {
    unsafe {
        match payload_opt(v) {
            Some(HeapObj::Map(_)) => {
                if let HeapObj::Map(m) = &mut *payload_mut(v) {
                    retain_locked(val);
                    if let Some(old) = m.insert(name.to_string(), val) {
                        release_locked(old);
                    }
                }
                Ok(val)
            }
            Some(HeapObj::PyObj(p)) => crate::pyffi::py_setattr(*p, name, val),
            Some(HeapObj::Instance { def, .. }) => {
                let info = struct_info(*def)
                    .ok_or_else(|| "instance with invalid type descriptor".to_string())?;
                let Some(&i) = info.index.get(name) else {
                    return Err(format!(
                        "instance of {} has no field \"{}\"",
                        info.name, name
                    ));
                };
                let val = match coerce_field(val, &info.fields[i]) {
                    Ok(x) => x,
                    Err(e) => return Err(format!("{}.{}\n", info.name, e)),
                };
                if let HeapObj::Instance { fields, .. } = &mut *payload_mut(v) {
                    retain_locked(val);
                    let old = std::mem::replace(&mut fields[i], val);
                    release_locked(old);
                }
                Ok(val)
            }
            _ => Err(format!(
                "cannot set member \"{}\" on value of type {}",
                name,
                kind_name(v)
            )),
        }
    }
}

/// slice with optional bounds; negative counts from the end.
pub fn slice(v: V, start: Option<i64>, end: Option<i64>) -> OpResult {
    unsafe {
        let norm = |i: i64, len: i64| -> i64 {
            let mut x = if i < 0 { i + len } else { i };
            if x < 0 {
                x = 0;
            }
            if x > len {
                x = len;
            }
            x
        };
        match payload_opt(v) {
            Some(HeapObj::Array(items)) => {
                let len = items.len() as i64;
                let s = norm(start.unwrap_or(0), len);
                let e = norm(end.unwrap_or(len), len);
                if s > e {
                    return Ok(mk_array(vec![]));
                }
                Ok(mk_array(items[s as usize..e as usize].to_vec()))
            }
            Some(HeapObj::Str(st)) => {
                let chars: Vec<char> = st.chars().collect();
                let len = chars.len() as i64;
                let s = norm(start.unwrap_or(0), len);
                let e = norm(end.unwrap_or(len), len);
                if s > e {
                    return Ok(mk_str_from(""));
                }
                Ok(mk_string(chars[s as usize..e as usize].iter().collect()))
            }
            _ => Err(format!("cannot slice value of type {}", kind_name(v))),
        }
    }
}

// ---------------------------------------------------------------------------
// function calls (native dispatch)
// ---------------------------------------------------------------------------

/// Callback implemented by the interpreter (and the native runner) so that
/// builtins like `map`, `sort_by`, and `net.serve` can call Plix functions.
pub trait Caller {
    fn call(&mut self, f: V, args: &[V]) -> OpResult;
}

thread_local! {
    static CALL_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}
pub const MAX_CALL_DEPTH: u32 = 1024;

pub struct DepthGuard;
impl DepthGuard {
    pub fn enter() -> Result<DepthGuard, String> {
        let d = CALL_DEPTH.with(|c| {
            let v = c.get();
            c.set(v + 1);
            v
        });
        if d >= MAX_CALL_DEPTH {
            CALL_DEPTH.with(|c| c.set(d));
            return Err(format!(
                "maximum recursion depth exceeded ({} calls)",
                MAX_CALL_DEPTH
            ));
        }
        Ok(DepthGuard)
    }
}
impl Drop for DepthGuard {
    fn drop(&mut self) {
        CALL_DEPTH.with(|c| c.set(c.get().saturating_sub(1)));
    }
}

/// Caller used by native code: everything is dispatched in this crate; AST
/// closures (interpreter-only) are rejected to avoid infinite delegation.
pub struct NativeCaller;
impl Caller for NativeCaller {
    fn call(&mut self, f: V, args: &[V]) -> OpResult {
        unsafe {
            if is_ptr(f) {
                if let HeapObj::ClsAst { .. } = payload(f) {
                    return Err(
                        "cannot invoke an interpreter (AST) closure from native code".into(),
                    );
                }
            }
        }
        call_value(self, f, args)
    }
}

/// Dispatch a call on an arbitrary Plix value (native code path).
pub fn call_value(caller: &mut dyn Caller, f: V, args: &[V]) -> OpResult {
    let _guard = DepthGuard::enter()?;
    unsafe {
        match payload_opt(f) {
            Some(HeapObj::ClsNative { code, cells, name }) => {
                trace_push(name);
                let fp: extern "C" fn(*const V, *const V, i64) -> V = std::mem::transmute(*code);
                let r = fp(cells.as_ptr(), args.as_ptr(), args.len() as i64);
                trace_pop();
                if err_flag() {
                    Err(take_error().unwrap_or_default())
                } else {
                    Ok(r)
                }
            }
            Some(HeapObj::Builtin(id)) => crate::builtins::call_builtin(*id, caller, args),
            Some(HeapObj::PyObj(p)) => crate::pyffi::py_call_handle(*p, args),
            Some(HeapObj::PyBound(p, attr)) => crate::pyffi::py_call_bound(*p, attr, args),
            Some(HeapObj::ClsAst { .. }) => caller.call(f, args),
            Some(HeapObj::Bound { recv, f }) => {
                // bound method: call f(recv, *args)
                let mut a2: Vec<V> = Vec::with_capacity(args.len() + 1);
                a2.push(use_locked(*recv));
                a2.extend_from_slice(args);
                call_value(caller, *f, &a2)
            }
            Some(HeapObj::StructDef(info)) => {
                // Point(...) sugar: call the `new` associated function when
                // one exists (mirrors Point.new(...)); otherwise explain.
                match info.methods.get("new") {
                    Some(&nf) => call_value(caller, nf, args),
                    None => Err(format!(
                        "struct {} is not callable (construct it with {} {{ field: value, .. }})",
                        info.name, info.name
                    )),
                }
            }
            _ => Err(format!("value of type {} is not callable", kind_name(f))),
        }
    }
}

// ===========================================================================
// extern "C" wrappers (Cranelift ABI). Fallible ops signal via TLS error
// flag and return 0.
// ===========================================================================

macro_rules! wrap {
    ($name:ident, ($($arg:ident : $t:ty),*), $body:expr) => {
        #[no_mangle]
        pub extern "C" fn $name($($arg: $t),*) -> V {
            if err_flag() {
                return 0;
            }
            let r: OpResult = $body;
            match r {
                Ok(v) => v,
                Err(e) => {
                    set_error(e);
                    0
                }
            }
        }
    };
}

wrap!(plix_add, (a: V, b: V), add(a, b));
wrap!(plix_sub, (a: V, b: V), sub(a, b));
wrap!(plix_mul, (a: V, b: V), mul(a, b));
wrap!(plix_div, (a: V, b: V), div(a, b));
wrap!(plix_rem, (a: V, b: V), rem(a, b));
wrap!(plix_neg, (a: V), neg(a));
wrap!(plix_band, (a: V, b: V), band(a, b));
wrap!(plix_bor, (a: V, b: V), bor(a, b));
wrap!(plix_bxor, (a: V, b: V), bxor(a, b));
wrap!(plix_shl, (a: V, b: V), shl(a, b));
wrap!(plix_shr, (a: V, b: V), shr(a, b));
wrap!(plix_bitnot, (a: V), bitnot(a));
wrap!(plix_len, (a: V), length(a));
wrap!(plix_index, (a: V, b: V), index_get(a, b));
wrap!(plix_index_set, (a: V, b: V, c: V), index_set(a, b, c));

#[no_mangle]
pub extern "C" fn plix_not(a: V) -> V {
    bool_of(!truthy(a))
}

#[no_mangle]
pub extern "C" fn plix_truthy(a: V) -> i64 {
    if err_flag() {
        0
    } else if truthy(a) {
        1
    } else {
        0
    }
}

macro_rules! wrap_cmp {
    ($name:ident, $f:expr) => {
        #[no_mangle]
        pub extern "C" fn $name(a: V, b: V) -> i64 {
            if err_flag() {
                return 0;
            }
            let r: Result<bool, String> = $f(a, b);
            match r {
                Ok(x) => {
                    if x {
                        1
                    } else {
                        0
                    }
                }
                Err(e) => {
                    set_error(e);
                    0
                }
            }
        }
    };
}

wrap_cmp!(plix_eq, |a, b| Ok(values_eq(a, b)));
wrap_cmp!(plix_ne, |a, b| Ok(!values_eq(a, b)));
wrap_cmp!(plix_lt, |a, b| compare(a, b)
    .map(|o| o == std::cmp::Ordering::Less));
wrap_cmp!(plix_le, |a, b| compare(a, b)
    .map(|o| o != std::cmp::Ordering::Greater));
wrap_cmp!(plix_gt, |a, b| compare(a, b)
    .map(|o| o == std::cmp::Ordering::Greater));
wrap_cmp!(plix_ge, |a, b| compare(a, b)
    .map(|o| o != std::cmp::Ordering::Less));

#[no_mangle]
pub extern "C" fn plix_str_of(v: V) -> V {
    if err_flag() {
        return 0;
    }
    mk_string(to_display(v))
}

/// # Safety
/// For a positive `n`, `p` must point to at least `n` readable Plix values for the duration of this call.
#[no_mangle]
pub unsafe extern "C" fn plix_array_new(p: *const V, n: i64) -> V {
    if err_flag() {
        return 0;
    }
    let items = if n > 0 && !p.is_null() {
        unsafe { std::slice::from_raw_parts(p, n as usize).to_vec() }
    } else {
        Vec::new()
    };
    mk_array(items)
}

#[no_mangle]
pub extern "C" fn plix_map_new() -> V {
    mk_map(std::collections::HashMap::new())
}

/// # Safety
/// For a positive `klen`, `kp` must point to a readable UTF-8 byte range of `klen` bytes. `m` must be a live Plix map value.
#[no_mangle]
pub unsafe extern "C" fn plix_map_set(m: V, kp: *const std::ffi::c_char, klen: i64, val: V) -> V {
    if err_flag() {
        return 0;
    }
    let key = if kp.is_null() || klen <= 0 {
        String::new()
    } else {
        let s = unsafe { std::slice::from_raw_parts(kp as *const u8, klen as usize) };
        String::from_utf8_lossy(s).into_owned()
    };
    let r = member_set(m, &key, val);
    match r {
        Ok(_) => m,
        Err(e) => {
            set_error(e);
            0
        }
    }
}

/// # Safety
/// For a positive `nlen`, `np` must point to a readable UTF-8 byte range of `nlen` bytes.
#[no_mangle]
pub unsafe extern "C" fn plix_member(v: V, np: *const std::ffi::c_char, nlen: i64) -> V {
    if err_flag() {
        return 0;
    }
    let name = if np.is_null() || nlen <= 0 {
        String::new()
    } else {
        let s = unsafe { std::slice::from_raw_parts(np as *const u8, nlen as usize) };
        String::from_utf8_lossy(s).into_owned()
    };
    match member_get(v, &name) {
        Ok(x) => x,
        Err(e) => {
            set_error(e);
            0
        }
    }
}

/// # Safety
/// For a positive `nlen`, `np` must point to a readable UTF-8 byte range of `nlen` bytes.
#[no_mangle]
pub unsafe extern "C" fn plix_member_set(
    v: V,
    np: *const std::ffi::c_char,
    nlen: i64,
    val: V,
) -> V {
    if err_flag() {
        return 0;
    }
    let name = if np.is_null() || nlen <= 0 {
        String::new()
    } else {
        let s = unsafe { std::slice::from_raw_parts(np as *const u8, nlen as usize) };
        String::from_utf8_lossy(s).into_owned()
    };
    match member_set(v, &name, val) {
        Ok(x) => x,
        Err(e) => {
            set_error(e);
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn plix_slice(v: V, s: i64, has_s: i64, e: i64, has_e: i64) -> V {
    if err_flag() {
        return 0;
    }
    let st = if has_s != 0 { Some(s) } else { None };
    let en = if has_e != 0 { Some(e) } else { None };
    match slice(v, st, en) {
        Ok(x) => x,
        Err(msg) => {
            set_error(msg);
            0
        }
    }
}

/// # Safety
/// For a positive `nargs`, `args` must point to at least `nargs` readable Plix values for the duration of this call.
#[no_mangle]
pub unsafe extern "C" fn plix_call(f: V, args: *const V, nargs: i64) -> V {
    if err_flag() {
        return 0;
    }
    let argv: &[V] = if nargs > 0 && !args.is_null() {
        unsafe { std::slice::from_raw_parts(args, nargs as usize) }
    } else {
        &[]
    };
    let mut c = NativeCaller;
    match call_value(&mut c, f, argv) {
        Ok(v) => v,
        Err(e) => {
            set_error(e);
            0
        }
    }
}

/// for-in helper: produce an array suitable for index-iteration.
///   array  -> the same array (live view)
///   string -> array of single-character strings
///   object -> sorted array of keys
pub fn forin_iter(v: V) -> OpResult {
    unsafe {
        match payload_opt(v) {
            Some(HeapObj::Array(_)) => Ok(use_locked(v)),
            Some(HeapObj::Str(s)) => Ok(mk_array(
                s.chars().map(|c| mk_str_from(&c.to_string())).collect(),
            )),
            Some(HeapObj::Map(m)) => {
                let mut ks: Vec<String> = m.keys().cloned().collect();
                ks.sort();
                Ok(mk_array(ks.iter().map(|k| mk_str_from(k)).collect()))
            }
            _ => Err(format!("cannot iterate value of type {}", kind_name(v))),
        }
    }
}

#[no_mangle]
pub extern "C" fn plix_forin_iter(v: V) -> V {
    if err_flag() {
        return 0;
    }
    match forin_iter(v) {
        Ok(x) => x,
        Err(e) => {
            set_error(e);
            0
        }
    }
}

/// # Safety
/// For a positive `ncells`, `cells` must point to at least `ncells` readable Plix values. For a positive `namelen`, `namep` must point to a readable UTF-8 byte range.
#[no_mangle]
pub unsafe extern "C" fn plix_closure_new(
    code: u64,
    cells: *const V,
    ncells: i64,
    namep: *const std::ffi::c_char,
    namelen: i64,
) -> V {
    let cv: Vec<V> = if ncells > 0 && !cells.is_null() {
        unsafe { std::slice::from_raw_parts(cells, ncells as usize).to_vec() }
    } else {
        Vec::new()
    };
    let name = if namep.is_null() || namelen <= 0 {
        "<closure>".to_string()
    } else {
        let s = unsafe { std::slice::from_raw_parts(namep as *const u8, namelen as usize) };
        String::from_utf8_lossy(s).into_owned()
    };
    mk_cls_native(code as usize, cv, name)
}
