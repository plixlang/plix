#![allow(
    clippy::match_single_binding,
    clippy::explicit_counter_loop,
    reason = "The explicit forms make interpreter control-flow and argument diagnostics easier to audit."
)]
//! Plix tree-walking interpreter v0.9.5
//!
//! Shares all value semantics with native code through plixrt: numbers,
//! strings, containers, operators, builtins, modules, the Python bridge.
//! Lexical scopes are chains of environments; closures capture their
//! defining environment (like Python/Lox), with mutation visible through
//! the capture.

use crate::ast::*;
use plixrt::builtins;
use plixrt::heap::{self, NULL, V};
use plixrt::value::{self, Caller, DepthGuard, OpResult};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

// ---------------------------------------------------------------------------
// scopes & slots
// ---------------------------------------------------------------------------

pub struct Slot {
    pub v: V,
    pub mutable: bool,
}

#[derive(Default)]
pub struct Scope {
    vars: HashMap<String, Rc<RefCell<Slot>>>,
    parent: Option<Rc<RefCell<Scope>>>,
}

pub type Env = Rc<RefCell<Scope>>;

fn scope_get(env: &Env, name: &str) -> Option<Rc<RefCell<Slot>>> {
    let mut cur = env.clone();
    loop {
        let found = cur.borrow().vars.get(name).cloned();
        match found {
            Some(s) => return Some(s),
            None => {
                let parent = cur.borrow().parent.clone();
                {
                    let p = parent?;
                    cur = p
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// control flow & errors
// ---------------------------------------------------------------------------

pub enum Flow {
    None,
    Break,
    Continue,
    Return(V),
}

#[derive(Debug, Clone)]
pub struct RtErr {
    pub msg: String,
    pub line: u32,
    pub col: u32,
}

fn rterr(span: crate::token::Span, msg: impl Into<String>) -> RtErr {
    RtErr {
        msg: msg.into(),
        line: span.line,
        col: span.col,
    }
}

type R<T> = Result<T, RtErr>;

/// the dynamic int range (tagged 62-bit); typed int arithmetic must stay
/// inside it — anything beyond is a strict overflow error, in lockstep
/// with the native backend's int_checked
const INT_DOMAIN_MIN: i64 = -(1i64 << 62);
const INT_DOMAIN_MAX: i64 = (1i64 << 62) - 1;

/// Strict typed-int arithmetic used when the checker has proven both
/// operands are ints (FLAG_STRICT_INT_ARITH). Mirrors the native backend:
/// i64 overflow raises instead of the dynamic float promotion, so the
/// interpreter and the compiled binary are observably identical.
/// Returns None when the op or operand kinds are not strictly int-only
/// (caller falls back to the dynamic semantics).
fn strict_int_arith(op: BinOp, a: V, b: V) -> Option<Result<V, String>> {
    if !matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul) {
        return None;
    }
    if !(heap::is_int(a) && heap::is_int(b)) {
        return None;
    }
    let x = heap::as_int(a);
    let y = heap::as_int(b);
    let (r, what) = match op {
        BinOp::Add => (x.checked_add(y), "addition"),
        BinOp::Sub => (x.checked_sub(y), "subtraction"),
        _ => (x.checked_mul(y), "multiplication"),
    };
    Some(match r {
        // typed ints are *62-bit* exactly like dynamic ints: exceeding the
        // dynamic range is an overflow error, not a silent float promotion
        Some(v) if (INT_DOMAIN_MIN..=INT_DOMAIN_MAX).contains(&v) => Ok(heap::mk_int(v)),
        _ => Err(format!("integer overflow in typed int {}", what)),
    })
}

/// typed-slot boundary guard + representation conversion, mirroring the
/// native backend's unboxed-slot guards (same messages, same fire points).
/// `what` names the slot (variable name, `argument "n" of f`, ...).
fn guard_typed(v: V, flags: u8, what: &str) -> Result<V, String> {
    if flags & FLAG_GUARD_NULLABLE != 0 && heap::is_null(v) {
        return Ok(v);
    }
    if flags & FLAG_GUARD_INT != 0 {
        if !heap::is_int(v) {
            return Err(heap::guard_msg_int(what));
        }
        return Ok(v);
    }
    if flags & FLAG_GUARD_FLOAT != 0 {
        // rt is the single source of truth (widening rule + message)
        return heap::as_f64_checked(v).map(heap::mk_float);
    }
    if flags & FLAG_GUARD_BOOL != 0 {
        // native bool slots store the truthiness: match the representation
        return Ok(heap::bool_of(value::truthy(v)));
    }
    Ok(v)
}

/// Runtime's struct-field descriptor is intentionally erased. Keep generic
/// containers as their erased runtime kind, and erase nullable fields to `any`
/// because the static checker owns the `T?` contract.
fn runtime_field_ty_name(ty: &TypeExpr) -> String {
    match ty.name.as_str() {
        "Option" | "option" => String::new(),
        other => other.to_string(),
    }
}

/// guard bits implied by a type *annotation* (params, return types)
fn guard_flags_of_type(ty: &TypeExpr) -> u8 {
    match ty.name.as_str() {
        "int" => FLAG_GUARD_INT,
        "float" => FLAG_GUARD_FLOAT,
        "bool" => FLAG_GUARD_BOOL,
        "Option" | "option" if ty.args.len() == 1 => {
            FLAG_GUARD_NULLABLE | guard_flags_of_type(&ty.args[0])
        }
        _ => 0,
    }
}

// AST function registry for interpreter closures (thread-local, single
// interpreter thread)
thread_local! {
    static FN_TABLE: RefCell<Vec<Rc<FuncDef>>> = const { RefCell::new(Vec::new()) };
}

fn register_fn(f: &Rc<FuncDef>) -> u32 {
    FN_TABLE.with(|t| {
        let mut t = t.borrow_mut();
        t.push(f.clone());
        (t.len() - 1) as u32
    })
}
fn lookup_fn(id: u32) -> Option<Rc<FuncDef>> {
    FN_TABLE.with(|t| t.borrow().get(id as usize).cloned())
}

// ---------------------------------------------------------------------------
// interpreter
// ---------------------------------------------------------------------------

pub struct Interpreter {
    pub globals: Env,
    module_cache: HashMap<String, V>,
    pub base_dir: PathBuf,
    /// static type information of the current unit (impl resolution tables)
    pub tinfo: Rc<crate::typecheck::TypeInfo>,
}

impl Interpreter {
    pub fn new(base_dir: PathBuf) -> Interpreter {
        let globals = Rc::new(RefCell::new(Scope::default()));
        let it = Interpreter {
            globals,
            module_cache: HashMap::new(),
            base_dir,
            tinfo: Rc::new(crate::typecheck::TypeInfo::default()),
        };
        // install builtins + native modules + constants
        for (name, v) in builtins::build_global_entries() {
            it.define_global(&name, v, false);
        }
        it
    }

    fn define_global(&self, name: &str, v: V, mutable: bool) {
        heap::swap_var(NULL, v); // hold one ref in the slot
        self.globals
            .borrow_mut()
            .vars
            .insert(name.to_string(), Rc::new(RefCell::new(Slot { v, mutable })));
    }

    /// Bind (or rebind) a variable in the given scope.
    fn define_var(&self, env: &Env, name: &str, v: V, kind: VarKind) {
        let mutable = kind != VarKind::Const;
        let existing = env.borrow().vars.get(name).cloned();
        if let Some(slot) = existing {
            let mut sl = slot.borrow_mut();
            sl.v = heap::swap_var(sl.v, v);
            sl.mutable = mutable;
            return;
        }
        let held = heap::swap_var(NULL, v);
        env.borrow_mut().vars.insert(
            name.to_string(),
            Rc::new(RefCell::new(Slot { v: held, mutable })),
        );
    }

    pub fn run(&mut self, prog: &Program) -> Result<(), RtErr> {
        for s in &prog.stmts {
            self.exec_top(s)?;
        }
        Ok(())
    }

    /// REPL helper: evaluate a single expression in the global scope.
    pub fn eval_pub(&mut self, e: &Expr) -> Result<V, RtErr> {
        let cp = heap::arena_checkpoint();
        let env = self.globals.clone();
        let r = self.eval(e, &env);
        match r {
            Ok(v) => {
                let out = heap::use_var(v);
                heap::arena_rewind(cp);
                Ok(out)
            }
            Err(e2) => {
                heap::arena_rewind(cp);
                Err(e2)
            }
        }
    }

    fn exec_top(&mut self, s: &Stmt) -> Result<(), RtErr> {
        let cp = heap::arena_checkpoint();
        let r = self.exec(s, &self.globals.clone());
        heap::arena_rewind(cp);
        match r {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    // ---------------------------------------------------------------
    // statements
    // ---------------------------------------------------------------
    fn exec(&mut self, s: &Stmt, env: &Env) -> R<Flow> {
        match &s.node {
            StmtKind::Var {
                kind, name, value, ..
            } => {
                let cp = heap::arena_checkpoint();
                let v = self.eval(value, env)?;
                // annotated slot: same boundary guard/conversion as native
                let v = guard_typed(v, s.flags.get(), name).map_err(|m| rterr(s.span, m))?;
                self.define_var(env, name, v, *kind);
                heap::arena_rewind(cp);
                Ok(Flow::None)
            }
            StmtKind::Func(def) => {
                let cl = self.make_closure(def, env);
                self.define_var(env, &def.name, cl, VarKind::Const);
                Ok(Flow::None)
            }
            StmtKind::Struct { name, fields } => {
                // create the runtime type object; field defaults are
                // evaluated once, at declaration time
                let def = heap::structdef_new(name.clone());
                for f in fields {
                    let cp = heap::arena_checkpoint();
                    let (dval, has) = match &f.default {
                        Some(de) => (self.eval(de, env)?, true),
                        None => (NULL, false),
                    };
                    let tyn = f.ty.as_ref().map(runtime_field_ty_name).unwrap_or_default();
                    heap::structdef_add_field(def, &f.name, &tyn, dval, has);
                    heap::arena_rewind(cp);
                }
                self.define_var(env, name, def, VarKind::Const);
                Ok(Flow::None)
            }
            StmtKind::Impl {
                target,
                trait_name,
                methods,
            } => {
                let slot = match scope_get(env, target) {
                    Some(s) => s,
                    None => {
                        return Err(rterr(
                            s.span,
                            format!("impl target \"{}\" is not a known struct", target),
                        ));
                    }
                };
                let def_v = slot.borrow().v;
                match trait_name {
                    None => {
                        for m in methods {
                            let cl = self.make_closure(m, env);
                            heap::structdef_add_method(def_v, None, &m.name, cl);
                        }
                    }
                    Some(tn) => {
                        // the checker already resolved overridden + default
                        // methods for this (struct, trait) pair
                        let resolved = self
                            .tinfo
                            .structs
                            .get(target)
                            .and_then(|sm| sm.trait_impls.get(tn))
                            .cloned()
                            .unwrap_or_default();
                        for (mname, mdef) in &resolved {
                            let cl = self.make_closure(mdef, env);
                            heap::structdef_add_method(def_v, Some(tn), mname, cl);
                        }
                    }
                }
                Ok(Flow::None)
            }
            StmtKind::Trait { .. } => {
                // compile-time only: defaults are resolved into impls
                Ok(Flow::None)
            }
            StmtKind::Enum { name: _, variants } => {
                for vdef in variants {
                    if vdef.fields.is_empty() {
                        let v = heap::mk_variant(&vdef.name, NULL, false);
                        self.define_var(env, &vdef.name, v, VarKind::Const);
                    }
                }
                Ok(Flow::None)
            }
            StmtKind::Import {
                module,
                alias,
                python,
            } => {
                let v = self.do_import(module, *python, s.span)?;
                self.define_var(env, alias, v, VarKind::Const);
                Ok(Flow::None)
            }
            StmtKind::ExprStmt(e) => {
                let cp = heap::arena_checkpoint();
                let r = self.eval(e, env);
                heap::arena_rewind(cp);
                r?;
                Ok(Flow::None)
            }
            StmtKind::Block(stmts) => {
                let child = Rc::new(RefCell::new(Scope {
                    vars: HashMap::new(),
                    parent: Some(env.clone()),
                }));
                for st in stmts {
                    let cp = heap::arena_checkpoint();
                    let r = self.exec(st, &child);
                    heap::arena_rewind(cp);
                    match r? {
                        Flow::None => {}
                        f => return Ok(f),
                    }
                }
                Ok(Flow::None)
            }
            StmtKind::If { cond, then, els } => {
                let cp = heap::arena_checkpoint();
                let c = self.eval(cond, env)?;
                heap::arena_rewind(cp);
                if value::truthy(c) {
                    self.exec(then, env)
                } else if let Some(e) = els {
                    self.exec(e, env)
                } else {
                    Ok(Flow::None)
                }
            }
            StmtKind::While { cond, body } => {
                loop {
                    let cp = heap::arena_checkpoint();
                    let c = match self.eval(cond, env) {
                        Ok(c) => c,
                        Err(e) => {
                            heap::arena_rewind(cp);
                            return Err(e);
                        }
                    };
                    heap::arena_rewind(cp);
                    if !value::truthy(c) {
                        break;
                    }
                    let cp2 = heap::arena_checkpoint();
                    let f = self.exec(body, env);
                    heap::arena_rewind(cp2);
                    match f? {
                        Flow::None | Flow::Continue => {}
                        Flow::Break => break,
                        f => return Ok(f),
                    }
                }
                Ok(Flow::None)
            }
            StmtKind::ForC {
                init,
                cond,
                step,
                body,
            } => {
                let loop_env = Rc::new(RefCell::new(Scope {
                    vars: HashMap::new(),
                    parent: Some(env.clone()),
                }));
                if let Some(i) = init {
                    self.exec(i, &loop_env)?;
                }
                loop {
                    if let Some(c) = cond {
                        let cp = heap::arena_checkpoint();
                        let cv = match self.eval(c, &loop_env) {
                            Ok(v) => v,
                            Err(e) => {
                                heap::arena_rewind(cp);
                                return Err(e);
                            }
                        };
                        heap::arena_rewind(cp);
                        if !value::truthy(cv) {
                            break;
                        }
                    }
                    let cp = heap::arena_checkpoint();
                    match self.exec(body, &loop_env) {
                        Err(e) => {
                            heap::arena_rewind(cp);
                            return Err(e);
                        }
                        Ok(Flow::Break) => {
                            heap::arena_rewind(cp);
                            break;
                        }
                        Ok(flow @ Flow::Return(_)) => {
                            heap::arena_rewind(cp);
                            return Ok(flow);
                        }
                        Ok(Flow::None) | Ok(Flow::Continue) => {
                            if let Some(st) = step {
                                if let Err(e) = self.eval(st, &loop_env) {
                                    heap::arena_rewind(cp);
                                    return Err(e);
                                }
                            }
                            heap::arena_rewind(cp);
                        }
                    }
                }
                Ok(Flow::None)
            }
            StmtKind::ForIn {
                name, iter, body, ..
            } => {
                let cp = heap::arena_checkpoint();
                let it = match self.eval(iter, env) {
                    Ok(v) => v,
                    Err(e) => {
                        heap::arena_rewind(cp);
                        return Err(e);
                    }
                };
                let arr = match value::forin_iter(it) {
                    Ok(a) => a,
                    Err(e) => {
                        heap::arena_rewind(cp);
                        return Err(rterr(s.span, e));
                    }
                };
                // the arena rewind below frees the iterator value unless some
                // variable already owns it (a literal `[1,2,3]` has no owner):
                // pin it for the duration of the loop
                let arr_keep = heap::retain_plain(arr);
                heap::arena_rewind(cp);

                let loop_env = Rc::new(RefCell::new(Scope {
                    vars: HashMap::new(),
                    parent: Some(env.clone()),
                }));
                self.define_var(&loop_env, name, NULL, VarKind::Auto);
                let n = heap::array_ref(arr, |a| a.len());
                let slot = scope_get(&loop_env, name).unwrap();
                let mut result: Result<Flow, RtErr> = Ok(Flow::None);
                for i in 0..n {
                    let elem = heap::array_ref(arr, |a| a[i]);
                    let elem = heap::use_var(elem);
                    // typed loop variable: same guard as the native raw slot
                    let elem = match guard_typed(elem, s.flags.get(), "for-in element") {
                        Ok(g) => g,
                        Err(m) => {
                            result = Err(rterr(s.span, m));
                            break;
                        }
                    };
                    {
                        let mut sl = slot.borrow_mut();
                        sl.v = heap::swap_var(sl.v, elem);
                    }
                    let cp2 = heap::arena_checkpoint();
                    let f = self.exec(body, &loop_env);
                    heap::arena_rewind(cp2);
                    match f {
                        Ok(Flow::None) | Ok(Flow::Continue) => {}
                        Ok(Flow::Break) => break,
                        Ok(other) => {
                            result = Ok(other);
                            break;
                        }
                        Err(e) => {
                            result = Err(e);
                            break;
                        }
                    }
                }
                heap::release_plain(arr_keep);
                result
            }
            StmtKind::MatchStmt { subject, arms } => match self.exec_match(subject, arms, env)? {
                (flow, _) => Ok(flow),
            },
            StmtKind::Return(v) => {
                let cp = heap::arena_checkpoint();
                let r = match v {
                    Some(e) => self.eval(e, env),
                    None => Ok(NULL),
                };
                match r {
                    Ok(val) => {
                        // keep the return value alive across the rewind and
                        // the upcoming frame_pop handoff
                        let out = heap::retain_plain(val);
                        heap::arena_rewind(cp);
                        Ok(Flow::Return(out))
                    }
                    Err(e) => {
                        heap::arena_rewind(cp);
                        Err(e)
                    }
                }
            }
            StmtKind::Break => Ok(Flow::Break),
            StmtKind::Continue => Ok(Flow::Continue),
        }
    }

    fn exec_match(&mut self, subject: &Expr, arms: &[MatchArm], env: &Env) -> R<(Flow, V)> {
        let cp = heap::arena_checkpoint();
        let sv = match self.eval(subject, env) {
            Ok(v) => v,
            Err(e) => {
                heap::arena_rewind(cp);
                return Err(e);
            }
        };
        // keep subject alive for the whole match
        let sv = heap::use_var(sv);
        for arm in arms {
            for pat in &arm.pats {
                if let Some(bindings) = self.pattern_bindings(pat, sv) {
                    let arm_env = Rc::new(RefCell::new(Scope {
                        vars: HashMap::new(),
                        parent: Some(env.clone()),
                    }));
                    for (n, v) in bindings {
                        self.define_var(&arm_env, &n, heap::use_var(v), VarKind::Auto);
                    }
                    heap::arena_rewind(cp);
                    match &arm.body {
                        MatchBody::Expr(e) => {
                            let v = self.eval(e, &arm_env)?;
                            return Ok((Flow::None, v));
                        }
                        MatchBody::Block(stmts) => {
                            for st in stmts {
                                let cp2 = heap::arena_checkpoint();
                                let r = self.exec(st, &arm_env);
                                heap::arena_rewind(cp2);
                                match r? {
                                    Flow::None => {}
                                    f => return Ok((f, NULL)),
                                }
                            }
                            return Ok((Flow::None, NULL));
                        }
                    }
                }
            }
        }
        heap::arena_rewind(cp);
        Err(rterr(subject.span, "non-exhaustive match: no arm matched"))
    }

    fn pattern_bindings(&self, pat: &Pattern, sv: V) -> Option<Vec<(String, V)>> {
        match pat {
            Pattern::Wildcard => Some(Vec::new()),
            Pattern::Ident(n) => Some(vec![(n.clone(), sv)]),
            Pattern::Null => heap::is_null(sv).then(Vec::new),
            Pattern::Bool(b) => (sv == heap::bool_of(*b)).then(Vec::new),
            Pattern::Int(i) => value::values_eq(sv, heap::mk_int(*i)).then(Vec::new),
            Pattern::Float(f) => value::values_eq(sv, heap::mk_float_unchecked(*f)).then(Vec::new),
            Pattern::Str(s) => value::values_eq(sv, heap::mk_str_from(s)).then(Vec::new),
            Pattern::Variant(name, args) => {
                if name == "Some" {
                    if heap::is_null(sv) || args.len() > 1 {
                        return None;
                    }
                    if args.is_empty() {
                        return Some(Vec::new());
                    }
                    return self.pattern_bindings(&args[0], sv);
                }
                if !heap::variant_is(sv, name) {
                    return None;
                }
                let mut out = Vec::new();
                for (i, p) in args.iter().enumerate() {
                    let fv = heap::variant_field(sv, i);
                    let bs = self.pattern_bindings(p, fv)?;
                    out.extend(bs);
                }
                Some(out)
            }
        }
    }

    // ---------------------------------------------------------------
    // expressions
    // ---------------------------------------------------------------
    fn eval(&mut self, e: &Expr, env: &Env) -> R<V> {
        let sp = e.span;
        match &e.node {
            ExprKind::Null => Ok(NULL),
            ExprKind::Bool(b) => Ok(heap::bool_of(*b)),
            ExprKind::Int(i) => Ok(heap::mk_int(*i)),
            ExprKind::Float(f) => Ok(heap::mk_float_unchecked(*f)),
            ExprKind::Str(s) => Ok(heap::mk_str_from(s)),
            ExprKind::Ident(name) => match scope_get(env, name) {
                Some(slot) => Ok(heap::use_var(slot.borrow().v)),
                None => Err(rterr(sp, format!("undefined variable \"{}\"", name))),
            },
            ExprKind::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    out.push(self.eval(it, env)?);
                }
                Ok(heap::mk_array(out))
            }
            ExprKind::Object(props) => {
                let mut m = HashMap::new();
                for (k, vexpr) in props {
                    let v = self.eval(vexpr, env)?;
                    m.insert(k.clone(), v);
                }
                Ok(heap::mk_map(m))
            }
            ExprKind::Unary(op, inner) => {
                let v = self.eval(inner, env)?;
                let r = match op {
                    UnOp::Neg => value::neg(v),
                    UnOp::Not => Ok(heap::bool_of(!value::truthy(v))),
                    UnOp::BitNot => value::bitnot(v),
                };
                r.map_err(|m| rterr(sp, m))
            }
            ExprKind::Borrow { expr, .. } => {
                // runtime no-op: borrows are enforced statically in own-mode
                self.eval(expr, env)
            }
            ExprKind::Binary(op, a, b) => {
                let av = self.eval(a, env)?;
                let bv = self.eval(b, env)?;
                // checker-proven int arithmetic is strict (native parity):
                // i64 overflow raises instead of promoting to float
                if e.flags.get() & crate::ast::FLAG_STRICT_INT_ARITH != 0 {
                    if let Some(r) = strict_int_arith(*op, av, bv) {
                        return r.map_err(|m| rterr(sp, m));
                    }
                }
                let r =
                    match op {
                        BinOp::Add => value::add(av, bv),
                        BinOp::Sub => value::sub(av, bv),
                        BinOp::Mul => value::mul(av, bv),
                        BinOp::Div => value::div(av, bv),
                        BinOp::Mod => value::rem(av, bv),
                        BinOp::Eq => Ok(heap::bool_of(value::values_eq(av, bv))),
                        BinOp::Ne => Ok(heap::bool_of(!value::values_eq(av, bv))),
                        BinOp::Lt => value::compare(av, bv)
                            .map(|o| heap::bool_of(o == std::cmp::Ordering::Less)),
                        BinOp::Le => value::compare(av, bv)
                            .map(|o| heap::bool_of(o != std::cmp::Ordering::Greater)),
                        BinOp::Gt => value::compare(av, bv)
                            .map(|o| heap::bool_of(o == std::cmp::Ordering::Greater)),
                        BinOp::Ge => value::compare(av, bv)
                            .map(|o| heap::bool_of(o != std::cmp::Ordering::Less)),
                        BinOp::BAnd => value::band(av, bv),
                        BinOp::BOr => value::bor(av, bv),
                        BinOp::BXor => value::bxor(av, bv),
                        BinOp::Shl => value::shl(av, bv),
                        BinOp::Shr => value::shr(av, bv),
                    };
                r.map_err(|m| rterr(sp, m))
            }
            ExprKind::Logical(op, a, b) => {
                let av = self.eval(a, env)?;
                match op {
                    LogicalOp::And => {
                        if !value::truthy(av) {
                            return Ok(av);
                        }
                    }
                    LogicalOp::Or => {
                        if value::truthy(av) {
                            return Ok(av);
                        }
                    }
                }
                self.eval(b, env)
            }
            ExprKind::Ternary(c, a, b) => {
                let cv = self.eval(c, env)?;
                if value::truthy(cv) {
                    self.eval(a, env)
                } else {
                    self.eval(b, env)
                }
            }
            ExprKind::Assign { target, op, value } => {
                self.eval_assign(target, *op, value, env, sp, e.flags.get())
            }
            ExprKind::Call(callee, args) => {
                // v0.7 optimized: inline small closures where possible
                let f = self.eval(callee, env)?;
                let mut argv = Vec::with_capacity(args.len());
                for a in args {
                    argv.push(self.eval(a, env)?);
                }
                self.call_any(f, &argv).map_err(|e| match e {
                    CallErr::Rt(e) => e,
                    CallErr::Msg(m) => rterr(sp, m),
                })
            }
            ExprKind::Index(obj, idx) => {
                let o = self.eval(obj, env)?;
                let i = self.eval(idx, env)?;
                value::index_get(o, i).map_err(|m| rterr(sp, m))
            }
            ExprKind::Slice { obj, start, end } => {
                let o = self.eval(obj, env)?;
                let st = match start {
                    Some(sx) => Some(self.want_int(sx, env)?),
                    None => None,
                };
                let en = match end {
                    Some(ex) => Some(self.want_int(ex, env)?),
                    None => None,
                };
                value::slice(o, st, en).map_err(|m| rterr(sp, m))
            }
            ExprKind::Member(obj, name) => {
                let o = self.eval(obj, env)?;
                value::member_get(o, name).map_err(|m| rterr(sp, m))
            }
            ExprKind::StructLit { name, fields } => {
                let def_v = match scope_get(env, name) {
                    Some(slot) => {
                        let v = slot.borrow().v;
                        heap::use_var(v)
                    }
                    None => {
                        return Err(rterr(
                            sp,
                            format!("unknown struct \"{}\" (struct literal)", name),
                        ));
                    }
                };
                let mut pairs: Vec<(String, V)> = Vec::with_capacity(fields.len());
                for (fname, ve) in fields {
                    let v = self.eval(ve, env)?;
                    pairs.push((fname.clone(), v));
                }
                value::instantiate(def_v, pairs).map_err(|m| rterr(sp, m))
            }
            ExprKind::FuncLit(def) => Ok(self.make_closure(def, env)),
            ExprKind::Match { subject, arms } => {
                let (flow, v) = self.exec_match(subject, arms, env)?;
                match flow {
                    Flow::None => Ok(v),
                    _ => Err(rterr(
                        sp,
                        "break/continue/return cannot escape a match expression",
                    )),
                }
            }
        }
    }

    fn want_int(&mut self, e: &Expr, env: &Env) -> R<i64> {
        let v = self.eval(e, env)?;
        if heap::is_int(v) {
            Ok(heap::as_int(v))
        } else {
            Err(rterr(
                e.span,
                format!("slice bound must be int, got {}", heap::kind_name(v)),
            ))
        }
    }

    fn eval_assign(
        &mut self,
        target: &AssignTarget,
        op: AssignOp,
        value: &Expr,
        env: &Env,
        sp: crate::token::Span,
        flags: u8,
    ) -> R<V> {
        match target {
            AssignTarget::Ident(name) => {
                let slot = match scope_get(env, name) {
                    Some(s) => s,
                    None => return Err(rterr(sp, format!("undefined variable \"{}\"", name))),
                };
                if !slot.borrow().mutable {
                    return Err(rterr(sp, format!("cannot assign to const \"{}\"", name)));
                }
                let rhs = self.eval(value, env)?;
                // typed local: boundary guard/conversion exactly where the
                // native backend guards its unboxed slot
                let rhs = if op == AssignOp::Eq {
                    guard_typed(rhs, flags, name).map_err(|m| rterr(sp, m))?
                } else {
                    guard_typed(rhs, flags, "compound assignment").map_err(|m| rterr(sp, m))?
                };
                let newv = if op == AssignOp::Eq {
                    rhs
                } else {
                    let old = heap::use_var(slot.borrow().v);
                    self.apply_assign_op(op, old, rhs, sp, flags)?
                };
                let mut sl = slot.borrow_mut();
                sl.v = heap::swap_var(sl.v, newv);
                Ok(heap::use_var(sl.v))
            }
            AssignTarget::Index(oe, ie) => {
                let o = self.eval(oe, env)?;
                let i = self.eval(ie, env)?;
                let rhs = self.eval(value, env)?;
                let newv = if op == AssignOp::Eq {
                    rhs
                } else {
                    let old = value::index_get(o, i).map_err(|m| rterr(sp, m.clone()))?;
                    self.apply_assign_op(op, old, rhs, sp, flags)?
                };
                value::index_set(o, i, newv).map_err(|m| rterr(sp, m))
            }
            AssignTarget::Member(oe, name) => {
                let o = self.eval(oe, env)?;
                let rhs = self.eval(value, env)?;
                let newv = if op == AssignOp::Eq {
                    rhs
                } else {
                    let old = value::member_get(o, name).map_err(|m| rterr(sp, m.clone()))?;
                    self.apply_assign_op(op, old, rhs, sp, flags)?
                };
                value::member_set(o, name, newv).map_err(|m| rterr(sp, m))
            }
        }
    }

    fn apply_assign_op(
        &self,
        op: AssignOp,
        old: V,
        rhs: V,
        sp: crate::token::Span,
        flags: u8,
    ) -> R<V> {
        if flags & crate::ast::FLAG_STRICT_INT_ARITH != 0 {
            let bop = match op {
                AssignOp::Add => BinOp::Add,
                AssignOp::Sub => BinOp::Sub,
                AssignOp::Mul => BinOp::Mul,
                _ => BinOp::Mod,
            };
            if let Some(r) = strict_int_arith(bop, old, rhs) {
                return r.map_err(|m| rterr(sp, m));
            }
        }
        let r = match op {
            AssignOp::Add => value::add(old, rhs),
            AssignOp::Sub => value::sub(old, rhs),
            AssignOp::Mul => value::mul(old, rhs),
            AssignOp::Div => value::div(old, rhs),
            AssignOp::Mod => value::rem(old, rhs),
            AssignOp::Eq => unreachable!(),
        };
        r.map_err(|m| rterr(sp, m))
    }

    // ---------------------------------------------------------------
    // functions
    // ---------------------------------------------------------------
    fn make_closure(&mut self, def: &Rc<FuncDef>, env: &Env) -> V {
        let fn_id = register_fn(def);
        let env_raw = Box::into_raw(Box::new(env.clone())) as usize;
        heap::mk_cls_ast(fn_id, env_raw)
    }

    fn call_any(&mut self, f: V, args: &[V]) -> Result<V, CallErr> {
        let _guard = DepthGuard::enter().map_err(CallErr::msg)?;
        unsafe {
            if heap::is_ptr(f) {
                match heap::payload(f) {
                    heap::HeapObj::ClsAst { fn_id, env } => {
                        let def = lookup_fn(*fn_id)
                            .ok_or_else(|| CallErr::msg("stale function reference"))?;
                        let cap_env: &Env = &*(*env as *const Env);
                        return self.invoke_ast(&def, cap_env, args);
                    }
                    heap::HeapObj::Builtin(id) => {
                        return builtins::call_builtin(*id, self, args).map_err(CallErr::msg);
                    }
                    heap::HeapObj::PyObj(p) => {
                        return plixrt::pyffi::py_call_handle(*p, args).map_err(CallErr::msg);
                    }
                    heap::HeapObj::PyBound(p, n) => {
                        return plixrt::pyffi::py_call_bound(*p, n, args).map_err(CallErr::msg);
                    }
                    heap::HeapObj::Bound { recv, f } => {
                        // bound method: f(recv, *args)
                        let fv = heap::use_var(*f);
                        let mut a2: Vec<V> = Vec::with_capacity(args.len() + 1);
                        a2.push(heap::use_var(*recv));
                        a2.extend_from_slice(args);
                        return self.call_any(fv, &a2);
                    }
                    heap::HeapObj::StructDef(info) => {
                        // Vec2(...) sugar: call the `new` associated function
                        // (mirrors Vec2.new(...), same rule as the runtime)
                        match info.methods.get("new") {
                            Some(&nf) => return self.call_any(heap::use_var(nf), args),
                            None => {
                                return Err(CallErr::msg(format!(
                                    "struct {} is not callable (construct it with {} {{ field: value, .. }})",
                                    info.name, info.name
                                )));
                            }
                        }
                    }
                    _ => {}
                }
            }
            Err(CallErr::msg(format!(
                "value of type {} is not callable",
                heap::kind_name(f)
            )))
        }
    }

    fn invoke_ast(&mut self, def: &Rc<FuncDef>, cap_env: &Env, args: &[V]) -> Result<V, CallErr> {
        heap::frame_push();
        let local = Rc::new(RefCell::new(Scope {
            vars: HashMap::new(),
            parent: Some(cap_env.clone()),
        }));
        // bind parameters (defaults / rest)
        let mut ai = 0usize;
        for p in &def.params {
            let v = if p.rest {
                let rest: Vec<V> = args[ai.min(args.len())..].to_vec();
                heap::mk_array(rest)
            } else if ai < args.len() {
                args[ai]
            } else if let Some(d) = &p.default {
                match self.eval(d, &local) {
                    Ok(v) => v,
                    Err(e) => {
                        heap::frame_pop_return(NULL);
                        return Err(CallErr::Rt(e));
                    }
                }
            } else {
                heap::frame_pop_return(NULL);
                return Err(CallErr::msg(format!(
                    "{}: missing argument \"{}\" (expected {}, got {})",
                    def.name,
                    p.name,
                    def.params.len(),
                    args.len()
                )));
            };
            ai += 1;
            // typed parameter: the native backend unboxes+guards at the
            // callee boundary; enforce the identical guard here (gradual
            // typing = verify dynamically what was only known as Any)
            let v = if let Some(t) = &p.ty {
                match guard_typed(
                    v,
                    guard_flags_of_type(t),
                    &format!("argument \"{}\" of {}", p.name, def.name),
                ) {
                    Ok(g) => g,
                    Err(m) => {
                        heap::frame_pop_return(NULL);
                        return Err(CallErr::msg(m));
                    }
                }
            } else {
                v
            };
            self.define_var(&local, &p.name, heap::use_var(v), VarKind::Auto);
        }
        if !def.params.iter().any(|p| p.rest) && args.len() > def.params.len() {
            let r = Err(CallErr::msg(format!(
                "{}: expected {} argument(s), got {}",
                def.name,
                def.params.len(),
                args.len()
            )));
            heap::frame_pop_return(NULL);
            return r;
        }
        heap::trace_push(&def.name);
        let mut result = Ok(NULL);
        for s in &def.body {
            let cp = heap::arena_checkpoint();
            let r = self.exec(s, &local);
            heap::arena_rewind(cp);
            match r {
                Ok(Flow::None) => {}
                Ok(Flow::Return(v)) => {
                    // declared return type: guard at the return boundary,
                    // exactly where the native backend enforces it. v is
                    // plain-owned (see Return): a converted replacement must
                    // be escaped, the original released.
                    match &def.ret_ty {
                        Some(t) => {
                            match guard_typed(
                                v,
                                guard_flags_of_type(t),
                                &format!("return value of {}", def.name),
                            ) {
                                Ok(g) => {
                                    if g != v {
                                        heap::retain_plain(g);
                                        heap::release_plain(v);
                                    }
                                    result = Ok(g);
                                }
                                Err(m) => {
                                    heap::release_plain(v);
                                    result = Err(CallErr::msg(m));
                                }
                            }
                        }
                        None => result = Ok(v),
                    }
                    break;
                }
                Ok(Flow::Break) | Ok(Flow::Continue) => {
                    result = Err(CallErr::msg(format!(
                        "break/continue outside of loop (in {})",
                        def.name
                    )));
                    break;
                }
                Err(e) => {
                    result = Err(CallErr::Rt(e));
                    break;
                }
            }
        }
        heap::trace_pop();
        let ret = result.unwrap_or_else(|e| {
            heap::set_error(match e {
                CallErr::Msg(m) => m,
                CallErr::Rt(re) => format!("{} (at {}:{})", re.msg, re.line, re.col),
            });
            NULL
        });
        let failed = heap::err_flag();
        heap::frame_pop_return(ret);
        if failed {
            Err(CallErr::msg(heap::take_error().unwrap_or_default()))
        } else {
            Ok(ret)
        }
    }

    // ---------------------------------------------------------------
    // imports
    // ---------------------------------------------------------------
    fn do_import(&mut self, module: &str, python: bool, sp: crate::token::Span) -> R<V> {
        if python {
            return plixrt::pyffi::import(module).map_err(|m| rterr(sp, m));
        }
        // native stdlib module? alias it into the scope
        if builtins::global_names().contains(&module) && !module.ends_with(".px") {
            if let Some(s) = scope_get(&self.globals, module) {
                return Ok(heap::use_var(s.borrow().v));
            }
        }
        // plix source file
        let path = self.base_dir.join(module);
        let canon = std::fs::canonicalize(&path)
            .map_err(|e| rterr(sp, format!("cannot resolve module \"{}\": {}", module, e)))?;
        let key = canon.to_string_lossy().into_owned();
        if let Some(&v) = self.module_cache.get(&key) {
            return Ok(heap::use_var(v));
        }
        let src = std::fs::read_to_string(&canon)
            .map_err(|e| rterr(sp, format!("cannot read module \"{}\": {}", module, e)))?;
        let stmts = crate::parser::parse_file(&src).map_err(|pe| RtErr {
            msg: format!("in module {}: {}", module, pe.msg),
            line: pe.span.line,
            col: pe.span.col,
        })?;
        let mname = canon.display().to_string();
        let mtinfo = crate::typecheck::check_program(&stmts).map_err(|errs| {
            let rendered = crate::owncheck::format_errors(&errs, &src, &mname);
            rterr(sp, format!("in module {}:\n{}", module, rendered))
        })?;
        crate::owncheck::check_program(&stmts).map_err(|errs| {
            let rendered = crate::owncheck::format_errors(&errs, &src, &mname);
            rterr(sp, format!("in module {}:\n{}", module, rendered))
        })?;
        let mod_env: Env = Rc::new(RefCell::new(Scope {
            vars: HashMap::new(),
            parent: Some(self.globals.clone()),
        }));
        let saved_base = std::mem::replace(
            &mut self.base_dir,
            canon.parent().unwrap_or(Path::new(".")).to_path_buf(),
        );
        // the module executes against its OWN type information (its structs
        // and impl tables are separate from the importer's)
        let saved_tinfo = std::mem::replace(&mut self.tinfo, Rc::new(mtinfo));
        for s in &stmts {
            let cp = heap::arena_checkpoint();
            let r = self.exec(s, &mod_env);
            heap::arena_rewind(cp);
            if let Err(e) = r {
                self.base_dir = saved_base;
                self.tinfo = saved_tinfo;
                return Err(e);
            }
        }
        self.base_dir = saved_base;
        self.tinfo = saved_tinfo;
        // collect top-level bindings into the module object
        let mut m = HashMap::new();
        for (name, slot) in mod_env.borrow().vars.iter() {
            m.insert(name.clone(), heap::use_var(slot.borrow().v));
        }
        let mv = heap::mk_map(m);
        self.module_cache.insert(key, heap::swap_var(NULL, mv));
        Ok(mv)
    }
}

// ---------------------------------------------------------------------------
// Caller bridge (builtins -> interpreter functions)
// ---------------------------------------------------------------------------

pub enum CallErr {
    Msg(String),
    Rt(RtErr),
}
impl CallErr {
    fn msg(m: impl Into<String>) -> CallErr {
        CallErr::Msg(m.into())
    }
}

impl Caller for Interpreter {
    fn call(&mut self, f: V, args: &[V]) -> OpResult {
        self.call_any(f, args).map_err(|e| match e {
            CallErr::Msg(m) => m,
            CallErr::Rt(re) => format!("{} (at line {})", re.msg, re.line),
        })
    }
}

/// Run a complete program (parse + ownership check + interpret). Returns the
/// interpreter in case the caller wants to inspect globals (REPL).
pub fn run_program(
    src: &str,
    name: &str,
    _base_dir: PathBuf,
    it: &mut Interpreter,
) -> Result<(), String> {
    let stmts = crate::parser::parse_file(src).map_err(|e| {
        format!(
            "{}:{}:{}: syntax error: {}",
            name, e.span.line, e.span.col, e.msg
        )
    })?;
    let tinfo = crate::typecheck::check_program(&stmts)
        .map_err(|errs| crate::owncheck::format_errors(&errs, src, name))?;
    crate::owncheck::check_program(&stmts)
        .map_err(|errs| crate::owncheck::format_errors(&errs, src, name))?;
    it.tinfo = Rc::new(tinfo);
    let prog = Program {
        stmts,
        source_name: name.to_string(),
    };
    it.run(&prog)
        .map_err(|e| format!("{}:{}:{}: RuntimeError: {}", name, e.line, e.col, e.msg))
}

/// one test function outcome (`test_*` at file top level)
#[allow(dead_code)]
pub struct TestOutcome {
    pub name: String,
    pub line: u32,
    pub result: Result<(), String>,
}

/// `plix test` file runner: parse + check, execute top-level statements
/// (setup), then invoke every top-level `func test_*` with no arguments.
/// Setup failures are reported as a single outcome named "<top level>".
pub fn run_test_file(
    src: &str,
    name: &str,
    it: &mut Interpreter,
) -> Result<Vec<TestOutcome>, String> {
    let stmts = crate::parser::parse_file(src).map_err(|e| {
        format!(
            "{}:{}:{}: syntax error: {}",
            name, e.span.line, e.span.col, e.msg
        )
    })?;
    let tinfo = crate::typecheck::check_program(&stmts)
        .map_err(|errs| crate::owncheck::format_errors(&errs, src, name))?;
    crate::owncheck::check_program(&stmts)
        .map_err(|errs| crate::owncheck::format_errors(&errs, src, name))?;
    it.tinfo = Rc::new(tinfo);

    // collect test functions before execution
    let mut test_fns: Vec<(Rc<FuncDef>, u32)> = Vec::new();
    for s in &stmts {
        if let StmtKind::Func(def) = &s.node {
            if def.name.starts_with("test_") {
                test_fns.push((def.clone(), s.span.line));
            }
        }
    }

    // run the whole top level (fn definitions just bind; everything else
    // is allowed test setup). A top-level crash fails the file as a whole.
    let mut out: Vec<TestOutcome> = Vec::new();
    let prog = Program {
        stmts,
        source_name: name.to_string(),
    };
    if let Err(e) = it.run(&prog) {
        out.push(TestOutcome {
            name: "<top level>".to_string(),
            line: e.line,
            result: Err(format!(
                "{}:{}:{}: RuntimeError: {}",
                name, e.line, e.col, e.msg
            )),
        });
        return Ok(out);
    }

    for (def, line) in test_fns {
        let clos = it.make_closure(&def, &it.globals.clone());
        let res = match it.call_any(clos, &[]) {
            Ok(_) => Ok(()),
            Err(CallErr::Msg(m)) => Err(format!("{}:{}:0: RuntimeError: {}", name, line, m)),
            Err(CallErr::Rt(e)) => Err(format!(
                "{}:{}:{}: RuntimeError: {}",
                name, e.line, e.col, e.msg
            )),
        };
        out.push(TestOutcome {
            name: def.name.clone(),
            line,
            result: res,
        });
    }
    Ok(out)
}
