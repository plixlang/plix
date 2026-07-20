//! Python bridge for Plix — talks to CPython directly through its stable C
//! API, loaded at runtime with dlopen/dlsym. No pyo3, no build-time Python
//! headers, no link flags: `plix` binaries stay self-contained and detect
//! Python dynamically (env override: PLIX_PYTHON_LIB).
//!
//! Performance model: values that have a cheap native representation
//! (int / float / bool / string / list / dict) cross the boundary
//! converted; big or exotic objects (numpy arrays, torch modules, dataframes)
//! stay on the Python side as opaque handles — zero copying, so repeated
//! calls (e.g. `np.matmul(a, b)`) cost only the C call itself. Attribute
//! access is resolved once and cached in a `PyBound` value, so hot loops
//! never pay for repeated getattr round-trips.

use crate::heap::*;
use crate::value::OpResult;
use std::collections::HashMap;
use std::ffi::{c_char, c_void, CString};

type Ptr = *mut c_void;

#[allow(non_snake_case)]
struct Py {
    _handle: Ptr,
    InitializeEx: unsafe extern "C" fn(i32),
    IsInitialized: unsafe extern "C" fn() -> i32,
    GILState_Ensure: unsafe extern "C" fn() -> i32,
    GILState_Release: unsafe extern "C" fn(i32),
    IncRef: unsafe extern "C" fn(Ptr),
    DecRef: unsafe extern "C" fn(Ptr),
    Import_ImportModule: unsafe extern "C" fn(*const c_char) -> Ptr,
    Object_GetAttrString: unsafe extern "C" fn(Ptr, *const c_char) -> Ptr,
    Object_SetAttrString: unsafe extern "C" fn(Ptr, *const c_char, Ptr) -> i32,
    Object_HasAttrString: unsafe extern "C" fn(Ptr, *const c_char) -> i32,
    Object_CallObject: unsafe extern "C" fn(Ptr, Ptr) -> Ptr,
    Object_CallableCheck: unsafe extern "C" fn(Ptr) -> i32,
    Object_GetItem: unsafe extern "C" fn(Ptr, Ptr) -> Ptr,
    Object_SetItem: unsafe extern "C" fn(Ptr, Ptr, Ptr) -> i32,
    Object_Length: unsafe extern "C" fn(Ptr) -> i64,
    Object_Str: unsafe extern "C" fn(Ptr) -> Ptr,
    Object_Repr: unsafe extern "C" fn(Ptr) -> Ptr,
    Tuple_New: unsafe extern "C" fn(i64) -> Ptr,
    Tuple_SetItem: unsafe extern "C" fn(Ptr, i64, Ptr) -> i32,
    Tuple_GetItem: unsafe extern "C" fn(Ptr, i64) -> Ptr,
    Tuple_Size: unsafe extern "C" fn(Ptr) -> i64,
    List_New: unsafe extern "C" fn(i64) -> Ptr,
    List_SetItem: unsafe extern "C" fn(Ptr, i64, Ptr) -> i32,
    List_GetItem: unsafe extern "C" fn(Ptr, i64) -> Ptr,
    List_Size: unsafe extern "C" fn(Ptr) -> i64,
    Dict_New: unsafe extern "C" fn() -> Ptr,
    Dict_SetItemString: unsafe extern "C" fn(Ptr, *const c_char, Ptr) -> i32,
    Dict_Next: unsafe extern "C" fn(Ptr, *mut i64, *mut Ptr, *mut Ptr) -> i32,
    Long_FromLongLong: unsafe extern "C" fn(i64) -> Ptr,
    Long_AsLongLong: unsafe extern "C" fn(Ptr) -> i64,
    Float_FromDouble: unsafe extern "C" fn(f64) -> Ptr,
    Float_AsDouble: unsafe extern "C" fn(Ptr) -> f64,
    Bool_FromLong: unsafe extern "C" fn(i32) -> Ptr,
    Unicode_FromStringAndSize: unsafe extern "C" fn(*const c_char, i64) -> Ptr,
    Unicode_AsUTF8: unsafe extern "C" fn(Ptr) -> *const c_char,
    Run_SimpleString: unsafe extern "C" fn(*const c_char) -> i32,
    Run_String: unsafe extern "C" fn(*const c_char, i32, Ptr, Ptr) -> Ptr,
    Err_Occurred: unsafe extern "C" fn() -> Ptr,
    Err_Fetch: unsafe extern "C" fn(*mut Ptr, *mut Ptr, *mut Ptr),
    Err_Clear: unsafe extern "C" fn(),
    // type objects (global addresses)
    TyLong: Ptr,
    TyFloat: Ptr,
    TyBool: Ptr,
    TyList: Ptr,
    TyDict: Ptr,
    TyTuple: Ptr,
    TyUnicode: Ptr,
    NoneStruct: Ptr,
}

unsafe impl Send for Py {}
unsafe impl Sync for Py {}

#[cfg(unix)]
extern "C" {
    fn dlopen(filename: *const c_char, flags: i32) -> Ptr;
    fn dlsym(handle: Ptr, symbol: *const c_char) -> Ptr;
}

#[cfg(unix)]
const RTLD_NOW: i32 = 2;
#[cfg(unix)]
const RTLD_GLOBAL: i32 = 0x100;

#[cfg(unix)]
unsafe fn open_library(filename: *const c_char) -> Ptr {
    dlopen(filename, RTLD_NOW | RTLD_GLOBAL)
}

#[cfg(unix)]
unsafe fn get_symbol(handle: Ptr, symbol: *const c_char) -> Ptr {
    dlsym(handle, symbol)
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryA(lp_lib_file_name: *const c_char) -> Ptr;
    fn GetProcAddress(h_module: Ptr, lp_proc_name: *const c_char) -> Ptr;
}

#[cfg(windows)]
unsafe fn open_library(filename: *const c_char) -> Ptr {
    LoadLibraryA(filename)
}

#[cfg(windows)]
unsafe fn get_symbol(handle: Ptr, symbol: *const c_char) -> Ptr {
    GetProcAddress(handle, symbol)
}

static PY: std::sync::OnceLock<Option<Py>> = std::sync::OnceLock::new();
static INIT: std::sync::Once = std::sync::Once::new();

fn candidates() -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    if let Ok(p) = std::env::var("PLIX_PYTHON_LIB") {
        v.push(p);
    }

    #[cfg(windows)]
    {
        // LoadLibraryA searches the application directory, System32, the
        // current directory and PATH. Users can still override with
        // PLIX_PYTHON_LIB=C:\\path\\to\\python313.dll.
        for ver in ["313", "312", "311", "310", "39", "38"] {
            v.push(format!("python{}.dll", ver));
        }
        v.push("python3.dll".into());
    }

    #[cfg(target_os = "macos")]
    {
        for ver in ["3.13", "3.12", "3.11", "3.10", "3.9", "3.8"] {
            v.push(format!("libpython{}.dylib", ver));
            v.push(format!("/usr/local/lib/libpython{}.dylib", ver));
            v.push(format!("/opt/homebrew/lib/libpython{}.dylib", ver));
            v.push(format!(
                "/Library/Frameworks/Python.framework/Versions/{}/Python",
                ver
            ));
        }
        v.push("libpython3.dylib".into());
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        for ver in ["3.13", "3.12", "3.11", "3.10", "3.9", "3.8"] {
            v.push(format!("libpython{}.so.1.0", ver));
            v.push(format!("libpython{}.so", ver));
            v.push(format!("/usr/local/lib/libpython{}.so.1.0", ver));
            v.push(format!("/usr/local/lib/libpython{}.so", ver));
            v.push(format!("/usr/lib/libpython{}.so.1.0", ver));
            v.push(format!("/usr/lib/x86_64-linux-gnu/libpython{}.so.1.0", ver));
        }
        v.push("libpython3.so".into());
    }

    v
}

macro_rules! sym {
    ($h:expr, $name:literal) => {{
        let c = CString::new($name).unwrap();
        let p = unsafe { get_symbol($h, c.as_ptr()) };
        if p.is_null() {
            return None;
        }
        unsafe { std::mem::transmute(p) }
    }};
    ($h:expr, $name:literal, data) => {{
        let c = CString::new($name).unwrap();
        let p = unsafe { get_symbol($h, c.as_ptr()) };
        if p.is_null() {
            return None;
        }
        p
    }};
}

fn load() -> Option<Py> {
    for path in candidates() {
        let c = match CString::new(path.clone()) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let h = unsafe { open_library(c.as_ptr()) };
        if h.is_null() {
            continue;
        }
        let api = Py {
            _handle: h,
            InitializeEx: sym!(h, "Py_InitializeEx"),
            IsInitialized: sym!(h, "Py_IsInitialized"),
            GILState_Ensure: sym!(h, "PyGILState_Ensure"),
            GILState_Release: sym!(h, "PyGILState_Release"),
            IncRef: sym!(h, "Py_IncRef"),
            DecRef: sym!(h, "Py_DecRef"),
            Import_ImportModule: sym!(h, "PyImport_ImportModule"),
            Object_GetAttrString: sym!(h, "PyObject_GetAttrString"),
            Object_SetAttrString: sym!(h, "PyObject_SetAttrString"),
            Object_HasAttrString: sym!(h, "PyObject_HasAttrString"),
            Object_CallObject: sym!(h, "PyObject_CallObject"),
            Object_CallableCheck: sym!(h, "PyCallable_Check"),
            Object_GetItem: sym!(h, "PyObject_GetItem"),
            Object_SetItem: sym!(h, "PyObject_SetItem"),
            Object_Length: sym!(h, "PyObject_Length"),
            Object_Str: sym!(h, "PyObject_Str"),
            Object_Repr: sym!(h, "PyObject_Repr"),
            Tuple_New: sym!(h, "PyTuple_New"),
            Tuple_SetItem: sym!(h, "PyTuple_SetItem"),
            Tuple_GetItem: sym!(h, "PyTuple_GetItem"),
            Tuple_Size: sym!(h, "PyTuple_Size"),
            List_New: sym!(h, "PyList_New"),
            List_SetItem: sym!(h, "PyList_SetItem"),
            List_GetItem: sym!(h, "PyList_GetItem"),
            List_Size: sym!(h, "PyList_Size"),
            Dict_New: sym!(h, "PyDict_New"),
            Dict_SetItemString: sym!(h, "PyDict_SetItemString"),
            Dict_Next: sym!(h, "PyDict_Next"),
            Long_FromLongLong: sym!(h, "PyLong_FromLongLong"),
            Long_AsLongLong: sym!(h, "PyLong_AsLongLong"),
            Float_FromDouble: sym!(h, "PyFloat_FromDouble"),
            Float_AsDouble: sym!(h, "PyFloat_AsDouble"),
            Bool_FromLong: sym!(h, "PyBool_FromLong"),
            Unicode_FromStringAndSize: sym!(h, "PyUnicode_FromStringAndSize"),
            Unicode_AsUTF8: sym!(h, "PyUnicode_AsUTF8"),
            Run_SimpleString: sym!(h, "PyRun_SimpleString"),
            Run_String: sym!(h, "PyRun_String"),
            Err_Occurred: sym!(h, "PyErr_Occurred"),
            Err_Fetch: sym!(h, "PyErr_Fetch"),
            Err_Clear: sym!(h, "PyErr_Clear"),
            TyLong: sym!(h, "PyLong_Type", data),
            TyFloat: sym!(h, "PyFloat_Type", data),
            TyBool: sym!(h, "PyBool_Type", data),
            TyList: sym!(h, "PyList_Type", data),
            TyDict: sym!(h, "PyDict_Type", data),
            TyTuple: sym!(h, "PyTuple_Type", data),
            TyUnicode: sym!(h, "PyUnicode_Type", data),
            NoneStruct: sym!(h, "_Py_NoneStruct", data),
        };
        return Some(api);
    }
    None
}

fn api() -> Result<&'static Py, String> {
    match PY.get_or_init(load) {
        Some(p) => Ok(p),
        None => Err("python runtime not available (libpython3 not found; \
                     set PLIX_PYTHON_LIB to the path of libpython3.x.so)"
            .to_string()),
    }
}

pub fn is_available() -> bool {
    PY.get_or_init(load).is_some()
}

struct GilGuard<'a> {
    state: i32,
    py: &'a Py,
}

impl<'a> Drop for GilGuard<'a> {
    fn drop(&mut self) {
        unsafe { (self.py.GILState_Release)(self.state) }
    }
}

fn gil() -> Result<GilGuard<'static>, String> {
    let py = api()?;
    INIT.call_once(|| unsafe {
        if (py.IsInitialized)() == 0 {
            (py.InitializeEx)(0);
        }
    });
    let state = unsafe { (py.GILState_Ensure)() };
    Ok(GilGuard { state, py })
}

/// read ob_type (PyObject: refcnt (isize) then type*)
unsafe fn type_of(py: &Py, p: Ptr) -> Ptr {
    let _ = py;
    let q = (p as *const u8).add(std::mem::size_of::<isize>()) as *const Ptr;
    *q
}

unsafe fn decref(py: &Py, p: Ptr) {
    if !p.is_null() {
        (py.DecRef)(p);
    }
}

fn fetch_err(py: &Py) -> String {
    unsafe {
        if (py.Err_Occurred)().is_null() {
            return "unknown python error".into();
        }
        let (mut t, mut v, mut tb) = (std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
        (py.Err_Fetch)(&mut t, &mut v, &mut tb);
        let msg = if !v.is_null() {
            let s = (py.Object_Str)(v);
            let m = py_string_to_rust(py, s);
            decref(py, s);
            m
        } else {
            "python exception".into()
        };
        decref(py, t);
        decref(py, v);
        decref(py, tb);
        (py.Err_Clear)();
        msg
    }
}

unsafe fn py_string_to_rust(py: &Py, s: Ptr) -> String {
    if s.is_null() {
        return String::new();
    }
    let c = (py.Unicode_AsUTF8)(s);
    if c.is_null() {
        return String::new();
    }
    std::ffi::CStr::from_ptr(c).to_string_lossy().into_owned()
}

// ---------------------------------------------------------------------------
// conversions
// ---------------------------------------------------------------------------

/// Convert a Plix value to a NEW PyObject reference.
unsafe fn plix_to_py(py: &Py, v: V) -> Result<Ptr, String> {
    if is_null(v) {
        let n = py.NoneStruct;
        (py.IncRef)(n);
        return Ok(n);
    }
    if v == TRUE {
        return Ok((py.Bool_FromLong)(1));
    }
    if v == FALSE {
        return Ok((py.Bool_FromLong)(0));
    }
    if is_int(v) {
        return Ok((py.Long_FromLongLong)(as_int(v)));
    }
    match payload(v) {
        HeapObj::Float(f) => Ok((py.Float_FromDouble)(*f)),
        HeapObj::Str(s) => {
            let bytes = s.as_bytes();
            Ok((py.Unicode_FromStringAndSize)(
                bytes.as_ptr() as *const c_char,
                bytes.len() as i64,
            ))
        }
        HeapObj::Array(items) => {
            let lst = (py.List_New)(items.len() as i64);
            if lst.is_null() {
                return Err(fetch_err(py));
            }
            for (i, &x) in items.iter().enumerate() {
                let px = plix_to_py(py, x)?;
                (py.List_SetItem)(lst, i as i64, px); // steals px
            }
            Ok(lst)
        }
        HeapObj::Map(m) => {
            let d = (py.Dict_New)();
            if d.is_null() {
                return Err(fetch_err(py));
            }
            for (k, &x) in m.iter() {
                let px = plix_to_py(py, x)?;
                let key = CString::new(k.clone()).unwrap_or_else(|_| CString::new("?").unwrap());
                let rc = (py.Dict_SetItemString)(d, key.as_ptr(), px);
                decref(py, px);
                if rc != 0 {
                    decref(py, d);
                    return Err(fetch_err(py));
                }
            }
            Ok(d)
        }
        HeapObj::PyObj(p) => {
            (py.IncRef)(*p);
            Ok(*p)
        }
        HeapObj::PyBound(..) => Err(
            "cannot pass a bound python attribute as a value (call it first)".into(),
        ),
        HeapObj::ClsNative { .. } | HeapObj::ClsAst { .. } | HeapObj::Builtin(_) => {
            Err("cannot pass Plix functions to python (v0.1)".into())
        }
        HeapObj::Cell(_) => Err("cannot pass internal cell to python".into()),
        // v0.3: structs/instances/bound methods stay Plix-side for now;
        // a deep-conversion helper can lower them to python dicts later.
        HeapObj::StructDef(_) => Err("cannot pass a struct type to python".into()),
        HeapObj::Instance { .. } => {
            Err("cannot pass a struct instance to python yet (use to_plix/map fields)".into())
        }
        HeapObj::Bound { .. } => Err("cannot pass a bound method to python".into()),
    }
}

/// Convert a PyObject to a Plix value. If `owned`, this function consumes
/// one reference (decref when wrapping); otherwise it increfs as needed.
unsafe fn py_to_plix(py: &Py, p: Ptr, owned: bool, depth: u32) -> Result<V, String> {
    if p.is_null() {
        return Err(fetch_err(py));
    }
    if depth > 64 {
        if owned {
            decref(py, p);
        }
        return Err("python object graph too deep".into());
    }
    let ty = type_of(py, p);
    // identity for exact-type checks; subclass fallthrough to conversion fns
    if p == py.NoneStruct {
        if owned {
            decref(py, p);
        }
        return Ok(NULL);
    }
    if ty == py.TyBool {
        let i = (py.Long_AsLongLong)(p);
        if owned {
            decref(py, p);
        }
        return Ok(bool_of(i != 0));
    }
    if ty == py.TyLong {
        let i = (py.Long_AsLongLong)(p);
        if owned {
            decref(py, p);
        }
        if err_flag() {
            return Err(take_error().unwrap_or_default());
        }
        return Ok(mk_int(i));
    }
    if ty == py.TyFloat {
        let f = (py.Float_AsDouble)(p);
        if owned {
            decref(py, p);
        }
        return Ok(mk_float_unchecked(f));
    }
    if ty == py.TyUnicode {
        let s = py_string_to_rust(py, p);
        if owned {
            decref(py, p);
        }
        return Ok(mk_string(s));
    }
    if ty == py.TyList || ty == py.TyTuple {
        let size = if ty == py.TyList {
            (py.List_Size)(p)
        } else {
            (py.Tuple_Size)(p)
        };
        let mut items = Vec::with_capacity(size.max(0) as usize);
        for i in 0..size {
            let it = if ty == py.TyList {
                (py.List_GetItem)(p, i)
            } else {
                (py.Tuple_GetItem)(p, i)
            };
            let v = py_to_plix(py, it, false, depth + 1)?;
            items.push(v);
        }
        if owned {
            decref(py, p);
        }
        return Ok(mk_array(items));
    }
    if ty == py.TyDict {
        let mut m: HashMap<String, V> = HashMap::new();
        let mut pos: i64 = 0;
        let mut k: Ptr = std::ptr::null_mut();
        let mut val: Ptr = std::ptr::null_mut();
        while (py.Dict_Next)(p, &mut pos, &mut k, &mut val) != 0 {
            let key = if !k.is_null() && type_of(py, k) == py.TyUnicode {
                py_string_to_rust(py, k)
            } else {
                let ks = (py.Object_Str)(k);
                let s = py_string_to_rust(py, ks);
                decref(py, ks);
                s
            };
            let pv = py_to_plix(py, val, false, depth + 1)?;
            m.insert(key, pv);
        }
        if owned {
            decref(py, p);
        }
        return Ok(mk_map(m));
    }
    // numpy-style scalar (np.int64, np.float64, torch 0-d tensors, ...):
    // objects exposing `item()` whose result is a cheap native type convert
    // automatically, so `arr.sum()` feels like a plain Plix number
    if let Some(v) = try_scalar_item(py, p, owned) {
        return Ok(v);
    }
    // heavy / exotic object: keep on the python side as an opaque handle
    if !owned {
        (py.IncRef)(p);
    }
    Ok(mk_pyobj(p))
}

/// try `p.item()` and convert when the result is int/float/bool/str;
/// returns None (clearing any python error) otherwise. Never consumes `p`:
/// on None the caller still owns its reference and keeps the handle path.
unsafe fn try_scalar_item(py: &Py, p: Ptr, owned: bool) -> Option<V> {
    let name = CString::new("item").ok()?;
    let attr = (py.Object_GetAttrString)(p, name.as_ptr());
    if attr.is_null() {
        (py.Err_Clear)();
        return None;
    }
    if (py.Object_CallableCheck)(attr) == 0 {
        decref(py, attr);
        return None;
    }
    let r = (py.Object_CallObject)(attr, std::ptr::null_mut());
    decref(py, attr);
    if r.is_null() {
        // not a scalar (e.g. a multi-element array): python raised; swallow
        (py.Err_Clear)();
        return None;
    }
    let ty = type_of(py, r);
    let cheap = ty == py.TyBool || ty == py.TyLong || ty == py.TyFloat || ty == py.TyUnicode;
    if cheap {
        let v = py_to_plix(py, r, true, 0).ok();
        if v.is_some() && owned {
            // scalar replaces the handle: drop the caller's reference
            decref(py, p);
        }
        return v;
    }
    decref(py, r);
    None
}

// ---------------------------------------------------------------------------
// public API
// ---------------------------------------------------------------------------

pub fn import(name: &str) -> OpResult {
    let py = api()?;
    let g = gil()?;
    let _ = &g;
    let cname = CString::new(name).map_err(|_| "py.import: bad module name")?;
    unsafe {
        let m = (py.Import_ImportModule)(cname.as_ptr());
        if m.is_null() {
            return Err(format!("py.import(\"{}\"): {}", name, fetch_err(py)));
        }
        Ok(mk_pyobj(m))
    }
}

/// Boolean probe: can this module be imported? Never raises into Plix and
/// clears the Python error state on failure.
pub fn has_module(name: &str) -> bool {
    let Ok(py) = api() else {
        return false;
    };
    let Ok(g) = gil() else {
        return false;
    };
    let _ = &g;
    let Ok(cname) = CString::new(name) else {
        return false;
    };
    unsafe {
        let m = (py.Import_ImportModule)(cname.as_ptr());
        if m.is_null() {
            (py.Err_Clear)();
            return false;
        }
        // module stays registered in sys.modules; drop only our reference
        decref(py, m);
        true
    }
}

pub fn eval(src: &str) -> OpResult {
    let py = api()?;
    let g = gil()?;
    let _ = &g;
    let csrc = CString::new(src).map_err(|_| "py.eval: source contains NUL byte")?;
    unsafe {
        let dict = (py.Dict_New)();
        if dict.is_null() {
            return Err(fetch_err(py));
        }
        const PY_EVAL_INPUT: i32 = 258;
        let r = (py.Run_String)(csrc.as_ptr(), PY_EVAL_INPUT, dict, dict);
        decref(py, dict);
        if r.is_null() {
            return Err(format!("py.eval: {}", fetch_err(py)));
        }
        py_to_plix(py, r, true, 0)
    }
}

pub fn exec(src: &str) -> Result<(), String> {
    let py = api()?;
    let g = gil()?;
    let _ = &g;
    let csrc = CString::new(src).map_err(|_| "py.exec: source contains NUL byte")?;
    unsafe {
        if (py.Run_SimpleString)(csrc.as_ptr()) != 0 {
            return Err(format!("py.exec: {}", fetch_err(py)));
        }
    }
    Ok(())
}

/// Call a raw callable handle with Plix args.
pub fn py_call_handle(callable: Ptr, args: &[V]) -> OpResult {
    let py = api()?;
    let g = gil()?;
    let _ = &g;
    unsafe {
        let t = (py.Tuple_New)(args.len() as i64);
        if t.is_null() {
            return Err(fetch_err(py));
        }
        for (i, &a) in args.iter().enumerate() {
            let pa = plix_to_py(py, a).map_err(|e| {
                (py.DecRef)(t);
                e
            })?;
            (py.Tuple_SetItem)(t, i as i64, pa);
        }
        let r = (py.Object_CallObject)(callable, t);
        decref(py, t);
        if r.is_null() {
            return Err(fetch_err(py));
        }
        py_to_plix(py, r, true, 0)
    }
}

pub fn py_call_bound(callable: Ptr, _name: &CString, args: &[V]) -> OpResult {
    py_call_handle(callable, args)
}

pub fn py_getattr(obj: Ptr, name: &str) -> OpResult {
    let py = api()?;
    let g = gil()?;
    let _ = &g;
    let cname = CString::new(name).map_err(|_| "python: bad attribute name")?;
    unsafe {
        let attr = (py.Object_GetAttrString)(obj, cname.as_ptr());
        if attr.is_null() {
            (py.Err_Clear)();
            return Err(format!("python object has no attribute \"{}\"", name));
        }
        Ok(mk_pybound(attr, cname))
    }
}

pub fn py_setattr(obj: Ptr, name: &str, val: V) -> OpResult {
    let py = api()?;
    let g = gil()?;
    let _ = &g;
    let cname = CString::new(name).map_err(|_| "python: bad attribute name")?;
    unsafe {
        let pv = plix_to_py(py, val)?;
        let rc = (py.Object_SetAttrString)(obj, cname.as_ptr(), pv);
        decref(py, pv);
        if rc != 0 {
            return Err(fetch_err(py));
        }
        Ok(val)
    }
}

pub fn hasattr(v: V, name: &str) -> OpResult {
    let py = api()?;
    let g = gil()?;
    let _ = &g;
    let cname = CString::new(name).map_err(|_| "python: bad attribute name")?;
    unsafe {
        let obj = match payload_opt_pub(v) {
            Some(HeapObj::PyObj(p)) => *p,
            _ => return Err("py.hasattr: expected a python object".into()),
        };
        Ok(bool_of((py.Object_HasAttrString)(obj, cname.as_ptr()) != 0))
    }
}

#[allow(static_mut_refs)]
unsafe fn payload_opt_pub(v: V) -> Option<&'static HeapObj> {
    if is_ptr(v) {
        Some(payload(v))
    } else {
        None
    }
}

pub fn py_index_get(obj: Ptr, idx: V) -> OpResult {
    let py = api()?;
    let g = gil()?;
    let _ = &g;
    unsafe {
        let key = plix_to_py(py, idx)?;
        let r = (py.Object_GetItem)(obj, key);
        decref(py, key);
        if r.is_null() {
            (py.Err_Clear)();
            // fall back to attribute access for string indices
            if let Some(HeapObj::Str(s)) = payload_opt_pub(idx) {
                return py_getattr(obj, &s.clone());
            }
            let _ = fetch_err(py);
            return Err("python: index not found".into());
        }
        py_to_plix(py, r, true, 0)
    }
}

pub fn py_index_set(obj: Ptr, idx: V, val: V) -> OpResult {
    let py = api()?;
    let g = gil()?;
    let _ = &g;
    unsafe {
        let key = plix_to_py(py, idx)?;
        let pv = plix_to_py(py, val)?;
        let rc = (py.Object_SetItem)(obj, key, pv);
        decref(py, key);
        decref(py, pv);
        if rc != 0 {
            return Err(fetch_err(py));
        }
        Ok(val)
    }
}

pub fn py_len(obj: Ptr) -> OpResult {
    let py = api()?;
    let g = gil()?;
    let _ = &g;
    unsafe {
        let n = (py.Object_Length)(obj);
        if n < 0 {
            return Err(fetch_err(py));
        }
        Ok(mk_int(n))
    }
}

/// Deep-convert a PyObj handle into native Plix values (lists→arrays,
/// dicts→objects, numpy scalars→numbers, ...). Non-handle values pass
/// through unchanged.
pub fn to_plix_deep(v: V) -> OpResult {
    let py = api()?;
    let g = gil()?;
    let _ = &g;
    unsafe {
        match payload_opt_pub(v) {
            Some(HeapObj::PyObj(p)) => {
                // try native conversion; if it stays a handle, attempt numpy's
                // .tolist()/item() conventions for arrays and scalars
                let r = py_to_plix(py, *p, false, 0)?;
                if is_ptr(r) {
                    if let Some(HeapObj::PyObj(_)) = payload_opt_pub(r) {
                        if let Ok(lst) = call_method0(py, *p, "tolist") {
                            return py_to_plix(py, lst, true, 0);
                        }
                        if let Ok(sc) = call_method0(py, *p, "item") {
                            return py_to_plix(py, sc, true, 0);
                        }
                    }
                }
                Ok(r)
            }
            _ => Ok(v),
        }
    }
}

unsafe fn call_method0(py: &Py, obj: Ptr, name: &str) -> Result<Ptr, String> {
    let cname = CString::new(name).unwrap();
    let attr = (py.Object_GetAttrString)(obj, cname.as_ptr());
    if attr.is_null() {
        (py.Err_Clear)();
        return Err("no such method".into());
    }
    let r = (py.Object_CallObject)(attr, std::ptr::null_mut());
    decref(py, attr);
    if r.is_null() {
        (py.Err_Clear)();
        return Err("method call failed".into());
    }
    Ok(r)
}

pub fn repr_val(v: V) -> String {
    unsafe {
        match payload_opt_pub(v) {
            Some(HeapObj::PyObj(p)) => py_repr_handle(*p),
            Some(HeapObj::PyBound(_, n)) => format!("<py attr {}>", n.to_string_lossy()),
            _ => crate::value::to_repr(v),
        }
    }
}

/// repr of a raw handle; safe when python is unavailable.
pub fn py_repr_handle(p: Ptr) -> String {
    let py = match api() {
        Ok(p) => p,
        Err(_) => return "<pyobject>".into(),
    };
    let g = match gil() {
        Ok(g) => g,
        Err(_) => return "<pyobject>".into(),
    };
    let _ = &g;
    unsafe {
        let s = (py.Object_Repr)(p);
        if s.is_null() {
            (py.Err_Clear)();
            return "<pyobject>".into();
        }
        let r = py_string_to_rust(py, s);
        decref(py, s);
        r
    }
}

/// decref a raw handle from the heap finalizer; never initializes python
/// just to destroy objects.
pub(crate) fn py_decref_locked(p: Ptr) {
    if p.is_null() {
        return;
    }
    let py = match PY.get() {
        Some(Some(p)) => p,
        _ => return, // python never loaded: leak (shutdown anyway)
    };
    unsafe {
        if (py.IsInitialized)() == 0 {
            return;
        }
        let state = (py.GILState_Ensure)();
        (py.DecRef)(p);
        (py.GILState_Release)(state);
    }
}
