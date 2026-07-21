#![allow(
    clippy::map_entry,
    reason = "The duplicate-method diagnostic needs to preserve the existing map entry unchanged."
)]
//! Plix gradual type checker (v0.3).
//!
//! Type annotations are optional: unannotated positions are `any` (the
//! dynamic top type) and behave exactly like v0.2. Where annotations are
//! present the checker is *strict*: a proven mismatch at an annotated
//! boundary is a hard compile error (E-codes match Rust's diagnostics
//! where possible: E0308 mismatched types, E0061 wrong arity, E0599 unknown
//! method, E0609 unknown field, E0046 missing trait item, E0277 trait not
//! implemented, E0594 mutation through const).
//!
//! Rules of the game:
//!   - `int` widens to `float`; `any` is compatible in both directions;
//!     other mismatches at typed boundaries are errors.
//!   - traits are structural bounds: a struct satisfies a trait annotation
//!     iff an `impl Trait for S` exists (checked at the impl, not the use).
//!   - `/` always yields float (runtime semantics), so in typed code it
//!     cannot feed an `int` slot.
//!   - function values carry signatures only when every parameter and the
//!     return are annotated ("fully typed"); otherwise calls through them
//!     are dynamic.

use crate::ast::*;
use crate::owncheck::OwnError;
use crate::token::Span;
use std::collections::HashMap;
use std::rc::Rc;

// ---------------------------------------------------------------------------
// types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    Any,
    Null,
    Int,
    Float,
    Str,
    Bool,
    Arr(Box<Ty>),
    Map(Box<Ty>, Box<Ty>),
    /// Nullable/Option type: `T?` or `Option<T>` accepts either `null` or `T`.
    Option(Box<Ty>),
    /// Built-in `Result<T, E>` sum type (`Ok(T)` | `Err(E)`).
    Result(Box<Ty>, Box<Ty>),
    Enum(String),
    Func(Rc<FnSig>),
    Struct(String),
    Trait(String),
    /// the struct TYPE object itself (value bound to `Point`), used to
    /// resolve associated functions: `Point.new` (never stored in locals)
    TypeVal(String),
}

pub fn ty_name(t: &Ty) -> String {
    match t {
        Ty::Any => "any".into(),
        Ty::Null => "null".into(),
        Ty::Int => "int".into(),
        Ty::Float => "float".into(),
        Ty::Str => "str".into(),
        Ty::Bool => "bool".into(),
        Ty::Arr(e) => format!("array<{}>", ty_name(e)),
        Ty::Map(k, v) => format!("map<{}, {}>", ty_name(k), ty_name(v)),
        Ty::Option(inner) => format!("{}?", ty_name(inner)),
        Ty::Result(ok, err) => format!("Result<{}, {}>", ty_name(ok), ty_name(err)),
        Ty::Enum(n) => n.clone(),
        Ty::Func(sig) if sig.ret.is_some() || sig.params.iter().any(|p| p.is_some()) => {
            let ps: Vec<String> = sig
                .params
                .iter()
                .map(|p| p.as_ref().map(ty_name).unwrap_or_else(|| "any".into()))
                .collect();
            format!(
                "({}) -> {}",
                ps.join(", "),
                sig.ret
                    .as_ref()
                    .map(ty_name)
                    .unwrap_or_else(|| "any".into())
            )
        }
        Ty::Func(_) => "func".into(),
        Ty::Struct(n) => n.clone(),
        Ty::Trait(n) => n.clone(),
        Ty::TypeVal(n) => format!("typeof {}", n),
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FnSig {
    /// parameter types in order (excluding the implicit `self` receiver
    /// for methods — method calls bind it from the receiver expression)
    pub params: Vec<Option<Ty>>,
    pub rest: bool,
    /// number of trailing params that have default values
    pub defaults: usize,
    pub ret: Option<Ty>,
}

impl FnSig {
    pub fn min_args(&self) -> usize {
        self.params.len().saturating_sub(self.defaults)
    }
    /// every parameter annotated + an annotated return type
    #[allow(dead_code)]
    pub fn fully_typed(&self) -> bool {
        self.ret.is_some() && self.params.iter().all(|p| p.is_some())
    }
}

#[derive(Debug, Clone)]
pub struct FieldMeta {
    pub ty: Ty,
    pub has_default: bool,
}

#[derive(Debug, Clone)]
pub struct MethodMeta {
    pub def: Rc<FuncDef>,
    pub sig: FnSig,
    /// &mut self receiver
    pub mutable: bool,
}

#[derive(Debug, Clone, Default)]
pub struct StructMeta {
    pub fields: HashMap<String, FieldMeta>,
    pub field_order: Vec<String>,
    pub methods: HashMap<String, MethodMeta>,
    /// trait name -> (method name -> def), defaults already resolved
    pub trait_impls: HashMap<String, HashMap<String, Rc<FuncDef>>>,
}

#[derive(Debug, Clone, Default)]
pub struct TraitMeta {
    /// required + defaulted methods (def.body empty => required)
    pub methods: HashMap<String, MethodMeta>,
    pub order: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct EnumMeta {
    pub variants: HashMap<String, Vec<Ty>>,
    pub order: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct TypeInfo {
    pub structs: HashMap<String, StructMeta>,
    pub traits: HashMap<String, TraitMeta>,
    pub enums: HashMap<String, EnumMeta>,
    /// signature of every function literal / declaration / method by id
    pub fn_sigs: HashMap<usize, FnSig>,
    /// unit-level (depth 0) functions by name
    pub top_fns: HashMap<String, Rc<FuncDef>>,
    /// fn id -> local names whose type is provably raw-able (int/float/bool)
    pub local_tys: HashMap<usize, HashMap<String, Ty>>,
}

// ---------------------------------------------------------------------------
// errors
// ---------------------------------------------------------------------------

fn terr(code: &'static str, msg: impl Into<String>, span: Span) -> OwnError {
    OwnError {
        code,
        msg: msg.into(),
        span,
        notes: Vec::new(),
    }
}

type CResult = Result<TypeInfo, Vec<OwnError>>;

// ---------------------------------------------------------------------------
// checker context
// ---------------------------------------------------------------------------

struct Ck {
    info: TypeInfo,
    errors: Vec<OwnError>,
    /// suppresses error emission (second inference pass of a function)
    silent: bool,
}

impl Ck {
    fn e(&mut self, err: OwnError) {
        if self.silent {
            return;
        }
        // dedupe identical diagnostics (the checker may evaluate the same
        // sub-expression twice, e.g. a method-call receiver)
        if self
            .errors
            .iter()
            .any(|x| x.code == err.code && x.msg == err.msg && x.span == err.span)
        {
            return;
        }
        self.errors.push(err);
    }

    // ---------------- TypeExpr -> Ty ----------------
    fn lower_ty(&mut self, te: &TypeExpr) -> Ty {
        let s = te.span;
        let no_args = |ck: &mut Ck, what: &str| {
            if !te.args.is_empty() {
                ck.e(terr(
                    "E0107",
                    format!("type {} takes no type arguments", what),
                    s,
                ));
            }
        };
        match te.name.as_str() {
            "any" => Ty::Any,
            "null" => {
                no_args(self, "null");
                Ty::Null
            }
            "int" => {
                no_args(self, "int");
                Ty::Int
            }
            "float" => {
                no_args(self, "float");
                Ty::Float
            }
            "str" => {
                no_args(self, "str");
                Ty::Str
            }
            "bool" => {
                no_args(self, "bool");
                Ty::Bool
            }
            "func" | "func<T,U>" => {
                if te.args.is_empty() {
                    Ty::Func(Rc::new(FnSig::default()))
                } else {
                    // Function type syntax `(A, B) -> R` is encoded by the
                    // parser as `func<A, B, R>` with the final arg as return.
                    let mut params: Vec<Option<Ty>> = Vec::new();
                    for a in &te.args[..te.args.len().saturating_sub(1)] {
                        params.push(Some(self.lower_ty(a)));
                    }
                    let ret = te.args.last().map(|r| self.lower_ty(r));
                    Ty::Func(Rc::new(FnSig {
                        params,
                        rest: false,
                        defaults: 0,
                        ret,
                    }))
                }
            }
            "Option" | "option" => {
                if te.args.len() != 1 {
                    self.e(terr("E0107", "Option takes exactly one type argument", s));
                    Ty::Option(Box::new(Ty::Any))
                } else {
                    Ty::Option(Box::new(self.lower_ty(&te.args[0])))
                }
            }
            "Result" | "result" => {
                if te.args.len() != 2 {
                    self.e(terr("E0107", "Result takes exactly two type arguments", s));
                    Ty::Result(Box::new(Ty::Any), Box::new(Ty::Any))
                } else {
                    Ty::Result(
                        Box::new(self.lower_ty(&te.args[0])),
                        Box::new(self.lower_ty(&te.args[1])),
                    )
                }
            }
            "array" => {
                if te.args.len() > 1 {
                    self.e(terr("E0107", "array takes at most one type argument", s));
                }
                let el = te.args.first().map(|a| self.lower_ty(a)).unwrap_or(Ty::Any);
                Ty::Arr(Box::new(el))
            }
            "map" => {
                if !te.args.is_empty() && te.args.len() != 2 {
                    self.e(terr("E0107", "map takes zero or two type arguments", s));
                }
                let mut args = te.args.iter();
                let k = args.next().map(|a| self.lower_ty(a)).unwrap_or(Ty::Any);
                let v = args.next().map(|a| self.lower_ty(a)).unwrap_or(Ty::Any);
                Ty::Map(Box::new(k), Box::new(v))
            }
            name => {
                if !te.args.is_empty() {
                    self.e(terr(
                        "E0107",
                        format!("{} is not generic (no type parameters)", name),
                        s,
                    ));
                }
                if self.info.structs.contains_key(name) {
                    Ty::Struct(name.to_string())
                } else if self.info.traits.contains_key(name) {
                    Ty::Trait(name.to_string())
                } else if self.info.enums.contains_key(name) {
                    Ty::Enum(name.to_string())
                } else {
                    self.e(terr(
                        "E0412",
                        format!("cannot find type \"{}\" in this scope", name),
                        s,
                    ));
                    Ty::Any
                }
            }
        }
    }

    // ---------------- signature of a FuncDef ----------------
    fn sig_of(&mut self, def: &FuncDef, self_ty: Option<&Ty>) -> FnSig {
        let mut params: Vec<Option<Ty>> = Vec::new();
        let mut rest = false;
        for p in &def.params {
            if p.rest {
                rest = true;
                continue;
            }
            if p.name == "self" && def.receiver.is_some() {
                continue; // receiver binds from the call receiver
            }
            params.push(p.ty.as_ref().map(|t| self.lower_ty(t)));
        }
        // `self` receiver: record the receiver type in the *checker only*
        // through self_ty; the signature seen by callers excludes it.
        let _ = self_ty;
        let defaults = def
            .params
            .iter()
            .filter(|p| p.default.is_some() && !p.rest)
            .count();
        let ret = def.ret_ty.as_ref().map(|t| self.lower_ty(t));
        FnSig {
            params,
            rest,
            defaults,
            ret,
        }
    }
}

// ---------------------------------------------------------------------------
// pass A: declarations
// ---------------------------------------------------------------------------

pub fn check_program(stmts: &[Stmt]) -> CResult {
    let mut ck = Ck {
        info: TypeInfo::default(),
        errors: Vec::new(),
        silent: false,
    };

    declare_structs_traits(&mut ck, stmts);
    fill_struct_fields(&mut ck, stmts);
    fill_traits(&mut ck, stmts);
    process_impls(&mut ck, stmts);
    collect_fn_sigs(&mut ck, stmts);

    // pass B: bodies
    let mut defs: Vec<Rc<FuncDef>> = Vec::new();
    collect_all_defs(stmts, &mut defs);
    for d in &defs {
        let self_ty = method_self_ty(&ck.info, d);
        let borrowed = ck.info.fn_sigs.get(&FuncDef::id(d)).cloned();
        ck.check_fn(d, self_ty, borrowed);
    }

    // top-level statements also need checking (struct defaults, literals)
    ck.check_toplevel(stmts);

    // name collisions: struct/trait vs top-level function
    let top_fn_list: Vec<(String, Span)> = ck
        .info
        .top_fns
        .iter()
        .map(|(n, f)| (n.clone(), f.span))
        .collect();
    for (name, span) in top_fn_list {
        if ck.info.structs.contains_key(&name) || ck.info.traits.contains_key(&name) {
            ck.e(terr(
                "E0428",
                format!(
                    "the name \"{}\" is defined multiple times (type and function)",
                    name
                ),
                span,
            ));
        }
    }

    if ck.errors.is_empty() {
        Ok(ck.info)
    } else {
        Err(ck.errors)
    }
}

/// find which struct an impl method belongs to (checker bookkeeping)
fn method_self_ty(info: &TypeInfo, def: &Rc<FuncDef>) -> Option<Ty> {
    def.receiver?;
    for (name, sm) in &info.structs {
        if let Some(mm) = sm.methods.get(&def.name) {
            if Rc::ptr_eq(&mm.def, def) {
                return Some(Ty::Struct(name.clone()));
            }
        }
        for tbl in sm.trait_impls.values() {
            if let Some(d2) = tbl.get(&def.name) {
                if Rc::ptr_eq(d2, def) {
                    return Some(Ty::Struct(name.clone()));
                }
            }
        }
    }
    None
}

fn declare_structs_traits(ck: &mut Ck, stmts: &[Stmt]) {
    for s in stmts {
        match &s.node {
            StmtKind::Struct { name, .. } => {
                if ck.info.structs.contains_key(name)
                    || ck.info.traits.contains_key(name)
                    || ck.info.enums.contains_key(name)
                {
                    ck.e(terr(
                        "E0428",
                        format!("the name \"{}\" is defined multiple times", name),
                        s.span,
                    ));
                } else {
                    ck.info.structs.insert(name.clone(), StructMeta::default());
                }
            }
            StmtKind::Trait { name, .. } => {
                if ck.info.structs.contains_key(name)
                    || ck.info.traits.contains_key(name)
                    || ck.info.enums.contains_key(name)
                {
                    ck.e(terr(
                        "E0428",
                        format!("the name \"{}\" is defined multiple times", name),
                        s.span,
                    ));
                } else {
                    ck.info.traits.insert(name.clone(), TraitMeta::default());
                }
            }
            StmtKind::Enum { name, variants } => {
                if ck.info.structs.contains_key(name)
                    || ck.info.traits.contains_key(name)
                    || ck.info.enums.contains_key(name)
                {
                    ck.e(terr(
                        "E0428",
                        format!("the name \"{}\" is defined multiple times", name),
                        s.span,
                    ));
                } else {
                    let mut em = EnumMeta::default();
                    for v in variants {
                        let fields: Vec<Ty> = v.fields.iter().map(|_| Ty::Any).collect();
                        if !fields.is_empty() {
                            ck.e(terr(
                                "E0658",
                                format!(
                                    "payload enum variant {} is experimental; use built-in Result<T,E> / Ok(v) / Err(e) for now",
                                    v.name
                                ),
                                v.span,
                            ));
                        }
                        em.order.push(v.name.clone());
                        em.variants.insert(v.name.clone(), fields);
                    }
                    ck.info.enums.insert(name.clone(), em);
                }
            }
            _ => {}
        }
    }
}

fn fill_struct_fields(ck: &mut Ck, stmts: &[Stmt]) {
    for s in stmts {
        if let StmtKind::Struct { name, fields } = &s.node {
            for f in fields {
                let ty = f.ty.as_ref().map(|t| ck.lower_ty(t)).unwrap_or(Ty::Any);
                let sm = ck.info.structs.get_mut(name).unwrap();
                if sm.fields.contains_key(&f.name) {
                    ck.e(terr(
                        "E0124",
                        format!(
                            "field \"{}\" is already declared in struct {}",
                            f.name, name
                        ),
                        f.span,
                    ));
                    continue;
                }
                sm.fields.insert(
                    f.name.clone(),
                    FieldMeta {
                        ty,
                        has_default: f.default.is_some(),
                    },
                );
                sm.field_order.push(f.name.clone());
            }
        }
    }
}

fn fill_traits(ck: &mut Ck, stmts: &[Stmt]) {
    for s in stmts {
        if let StmtKind::Trait { name, methods } = &s.node {
            let mut order = Vec::new();
            let mut metas: Vec<(String, MethodMeta)> = Vec::new();
            for m in methods {
                let mut sig = ck.sig_of(m, None);
                // trait methods need a receiver and a fully annotated shape
                // for useful checks (annotations themselves optional)
                let _ = &mut sig;
                let mutable = matches!(m.receiver, Some(Receiver::MutRef));
                if m.receiver.is_none() {
                    ck.e(terr(
                        "E0643",
                        format!("trait method \"{}\" must have a self receiver", m.name),
                        m.span,
                    ));
                }
                order.push(m.name.clone());
                metas.push((
                    m.name.clone(),
                    MethodMeta {
                        def: m.clone(),
                        sig,
                        mutable,
                    },
                ));
            }
            let mut dups: Vec<String> = Vec::new();
            {
                let tm = ck.info.traits.get_mut(name).unwrap();
                for (mname, mm) in metas {
                    if tm.methods.contains_key(&mname) {
                        dups.push(mname.clone());
                    } else {
                        tm.methods.insert(mname, mm);
                    }
                }
                tm.order = order;
            }
            for mname in dups {
                ck.e(terr(
                    "E0592",
                    format!("duplicate method \"{}\" in trait {}", mname, name),
                    s.span,
                ));
            }
        }
    }
}

fn process_impls(ck: &mut Ck, stmts: &[Stmt]) {
    for s in stmts {
        if let StmtKind::Impl {
            target,
            trait_name,
            methods,
        } = &s.node
        {
            if !ck.info.structs.contains_key(target) {
                ck.e(terr(
                    "E0412",
                    format!(
                        "cannot find struct \"{}\" in this scope (impl target)",
                        target
                    ),
                    s.span,
                ));
                continue;
            }
            match trait_name {
                None => {
                    // inherent impl
                    let mut sigs: Vec<(Rc<FuncDef>, FnSig, bool)> = Vec::new();
                    for m in methods {
                        let selft = Ty::Struct(target.clone());
                        let sig = ck.sig_of(m, Some(&selft));
                        let mutable = matches!(m.receiver, Some(Receiver::MutRef));
                        sigs.push((m.clone(), sig, mutable));
                    }
                    let mut dups: Vec<(String, Span)> = Vec::new();
                    {
                        let sm = ck.info.structs.get_mut(target).unwrap();
                        for (m, sig, mutable) in sigs {
                            if sm.methods.contains_key(&m.name) {
                                dups.push((m.name.clone(), m.span));
                            } else {
                                sm.methods.insert(
                                    m.name.clone(),
                                    MethodMeta {
                                        def: m.clone(),
                                        sig,
                                        mutable,
                                    },
                                );
                            }
                        }
                    }
                    for (mname, mspan) in dups {
                        ck.e(terr(
                            "E0592",
                            format!(
                                "duplicate definitions with name \"{}\" (impl {})",
                                mname, target
                            ),
                            mspan,
                        ));
                    }
                }
                Some(tn) => {
                    if !ck.info.traits.contains_key(tn) {
                        ck.e(terr(
                            "E0405",
                            format!("cannot find trait \"{}\" in this scope", tn),
                            s.span,
                        ));
                        continue;
                    }
                    // provided methods
                    let mut provided: HashMap<String, (Rc<FuncDef>, FnSig, bool)> = HashMap::new();
                    for m in methods {
                        let selft = Ty::Struct(target.clone());
                        let sig = ck.sig_of(m, Some(&selft));
                        let mutable = matches!(m.receiver, Some(Receiver::MutRef));
                        provided.insert(m.name.clone(), (m.clone(), sig, mutable));
                    }
                    // validate against the trait and resolve defaults
                    let trait_methods: Vec<(String, MethodMeta)> = ck.info.traits[tn]
                        .methods
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    let mut resolved: HashMap<String, Rc<FuncDef>> = HashMap::new();
                    for (mname, tmeta) in &trait_methods {
                        match provided.get(mname) {
                            Some((def, sig, mutable)) => {
                                // signature compatibility: same arity
                                let exp = &tmeta.sig;
                                if sig.params.len() != exp.params.len() {
                                    ck.e(terr(
                                        "E0053",
                                        format!(
                                            "method \"{}\" has {} parameter(s) but the declaration in trait {} has {}",
                                            mname,
                                            sig.params.len(),
                                            tn,
                                            exp.params.len()
                                        ),
                                        s.span,
                                    ));
                                }
                                if *mutable != tmeta.mutable {
                                    ck.e(terr(
                                        "E0053",
                                        format!(
                                            "method \"{}\" has a different receiver than in trait {} (&mut mismatch)",
                                            mname, tn
                                        ),
                                        s.span,
                                    ));
                                }
                                resolved.insert(mname.clone(), def.clone());
                            }
                            None => {
                                if tmeta.def.body.is_empty() {
                                    ck.e(terr(
                                        "E0046",
                                        format!(
                                            "not all trait items implemented, missing: \"{}\" (impl {} for {})",
                                            mname, tn, target
                                        ),
                                        s.span,
                                    ));
                                } else {
                                    resolved.insert(mname.clone(), tmeta.def.clone());
                                }
                            }
                        }
                    }
                    // methods not in the trait
                    for (mname, (def, _, _)) in &provided {
                        if !ck.info.traits[tn].methods.contains_key(mname) {
                            ck.e(terr(
                                "E0407",
                                format!("method \"{}\" is not a member of trait {}", mname, tn),
                                def.span,
                            ));
                        }
                    }
                    let sm = ck.info.structs.get_mut(target).unwrap();
                    if sm.trait_impls.contains_key(tn) {
                        ck.e(terr(
                            "E0119",
                            format!(
                                "conflicting implementations of trait {} for type {}",
                                tn, target
                            ),
                            s.span,
                        ));
                    } else {
                        sm.trait_impls.insert(tn.clone(), resolved);
                    }
                }
            }
        }
    }
}

fn collect_fn_sigs(ck: &mut Ck, stmts: &[Stmt]) {
    let mut defs: Vec<Rc<FuncDef>> = Vec::new();
    collect_all_defs(stmts, &mut defs);
    for d in &defs {
        let id = FuncDef::id(d);
        if ck.info.fn_sigs.contains_key(&id) {
            continue;
        }
        let self_ty = Some(Ty::Struct(String::new())); // receiver handled separately
        let sig = ck.sig_of(d, self_ty.as_ref().filter(|_| d.receiver.is_some()));
        ck.info.fn_sigs.insert(id, sig);
    }
    // unit-level functions by name
    for s in stmts {
        if let StmtKind::Func(f) = &s.node {
            if let Some(prev) = ck.info.top_fns.get(&f.name) {
                // methods are never in top_fns; only plain fns
                if prev.receiver.is_none() {
                    ck.e(terr(
                        "E0428",
                        format!("function \"{}\" is defined multiple times", f.name),
                        f.span,
                    ));
                }
            }
            if f.receiver.is_none() {
                ck.info.top_fns.insert(f.name.clone(), f.clone());
            }
        }
    }
}

/// collect every FuncDef in the unit (decls, methods, trait defaults, literals)
pub fn collect_all_defs(stmts: &[Stmt], out: &mut Vec<Rc<FuncDef>>) {
    fn rec_s(s: &Stmt, out: &mut Vec<Rc<FuncDef>>) {
        match &s.node {
            StmtKind::Func(f) => {
                out.push(f.clone());
                rec_stmts(&f.body, out);
            }
            StmtKind::Impl { methods, .. } => {
                for m in methods {
                    out.push(m.clone());
                    rec_stmts(&m.body, out);
                }
            }
            StmtKind::Trait { methods, .. } => {
                for m in methods {
                    out.push(m.clone());
                    rec_stmts(&m.body, out);
                }
            }
            StmtKind::Struct { fields, .. } => {
                for f in fields {
                    if let Some(d) = &f.default {
                        rec_e(d, out);
                    }
                }
            }
            StmtKind::Var { value, .. } => rec_e(value, out),
            StmtKind::ExprStmt(e) => rec_e(e, out),
            StmtKind::Block(b) => rec_stmts(b, out),
            StmtKind::If { cond, then, els } => {
                rec_e(cond, out);
                rec_s(then, out);
                if let Some(e) = els {
                    rec_s(e, out);
                }
            }
            StmtKind::While { cond, body } => {
                rec_e(cond, out);
                rec_s(body, out);
            }
            StmtKind::ForC {
                init,
                cond,
                step,
                body,
            } => {
                if let Some(i) = init {
                    rec_s(i, out);
                }
                if let Some(c) = cond {
                    rec_e(c, out);
                }
                if let Some(st) = step {
                    rec_e(st, out);
                }
                rec_s(body, out);
            }
            StmtKind::ForIn { iter, body, .. } => {
                rec_e(iter, out);
                rec_s(body, out);
            }
            StmtKind::MatchStmt { subject, arms } => {
                rec_e(subject, out);
                for a in arms {
                    match &a.body {
                        MatchBody::Expr(e) => rec_e(e, out),
                        MatchBody::Block(b) => rec_stmts(b, out),
                    }
                }
            }
            StmtKind::Return(e) => {
                if let Some(x) = e {
                    rec_e(x, out);
                }
            }
            StmtKind::Enum { .. }
            | StmtKind::Import { .. }
            | StmtKind::Break
            | StmtKind::Continue => {}
        }
    }
    fn rec_stmts(stmts: &[Stmt], out: &mut Vec<Rc<FuncDef>>) {
        for s in stmts {
            rec_s(s, out);
        }
    }
    fn rec_e(e: &Expr, out: &mut Vec<Rc<FuncDef>>) {
        match &e.node {
            ExprKind::FuncLit(f) => {
                out.push(f.clone());
                rec_stmts(&f.body, out);
            }
            ExprKind::Array(xs) => {
                for x in xs {
                    rec_e(x, out);
                }
            }
            ExprKind::Object(ps) => {
                for (_, x) in ps {
                    rec_e(x, out);
                }
            }
            ExprKind::StructLit { fields, .. } => {
                for (_, x) in fields {
                    rec_e(x, out);
                }
            }
            ExprKind::Unary(_, x) | ExprKind::Borrow { expr: x, .. } => rec_e(x, out),
            ExprKind::Binary(_, a, b) | ExprKind::Logical(_, a, b) => {
                rec_e(a, out);
                rec_e(b, out);
            }
            ExprKind::Ternary(a, b, c) => {
                rec_e(a, out);
                rec_e(b, out);
                rec_e(c, out);
            }
            ExprKind::Assign { target, value, .. } => {
                rec_e(value, out);
                match target {
                    AssignTarget::Index(o, i) => {
                        rec_e(o, out);
                        rec_e(i, out);
                    }
                    AssignTarget::Member(o, _) => rec_e(o, out),
                    AssignTarget::Ident(_) => {}
                }
            }
            ExprKind::Call(c, xs) => {
                rec_e(c, out);
                for x in xs {
                    rec_e(x, out);
                }
            }
            ExprKind::Index(a, b) => {
                rec_e(a, out);
                rec_e(b, out);
            }
            ExprKind::Slice { obj, start, end } => {
                rec_e(obj, out);
                if let Some(s) = start {
                    rec_e(s, out);
                }
                if let Some(e2) = end {
                    rec_e(e2, out);
                }
            }
            ExprKind::Member(o, _) => rec_e(o, out),
            ExprKind::Match { subject, arms } => {
                rec_e(subject, out);
                for a in arms {
                    if let MatchBody::Expr(ex) = &a.body {
                        rec_e(ex, out);
                    }
                }
            }
            _ => {}
        }
    }
    rec_stmts(stmts, out);
}

// ---------------------------------------------------------------------------
// pass B: body checking
// ---------------------------------------------------------------------------

/// out-of-line so check_program stays readable
impl Ck {
    /// expected <- found: is `found` assignable to a slot of type `expected`?
    fn assignable(&self, expected: &Ty, found: &Ty) -> bool {
        match (expected, found) {
            (Ty::Any, _) | (_, Ty::Any) => true,
            (a, b) if a == b => true,
            (Ty::Float, Ty::Int) => true,
            (Ty::Option(_), Ty::Null) => true,
            (Ty::Option(inner), found) => self.assignable(inner, found),
            (Ty::Result(a1, b1), Ty::Result(a2, b2)) => {
                self.assignable(a1, a2) && self.assignable(b1, b2)
            }
            (Ty::Arr(a), Ty::Arr(b)) => self.assignable(a, b),
            (Ty::Map(a1, b1), Ty::Map(a2, b2)) => {
                self.assignable(a1, a2) && self.assignable(b1, b2)
            }
            (Ty::Func(exp), Ty::Func(found)) => {
                if exp.params.is_empty() && exp.ret.is_none() {
                    true
                } else {
                    exp.params.len() == found.params.len()
                        && exp
                            .params
                            .iter()
                            .zip(found.params.iter())
                            .all(|(a, b)| match (a, b) {
                                (Some(a), Some(b)) => {
                                    self.assignable(a, b) && self.assignable(b, a)
                                }
                                (None, _) => true,
                                (Some(_), None) => false,
                            })
                        && match (&exp.ret, &found.ret) {
                            (Some(a), Some(b)) => self.assignable(a, b),
                            (None, _) => true,
                            (Some(_), None) => false,
                        }
                }
            }
            (Ty::Trait(t), Ty::Struct(s)) => self
                .info
                .structs
                .get(s)
                .map(|sm| sm.trait_impls.contains_key(t))
                .unwrap_or(false),
            _ => false,
        }
    }

    fn must_assign(&mut self, expected: &Ty, found: &Ty, span: Span, what: &str) {
        if self.assignable(expected, found) {
            return;
        }
        // trait violation gets the dedicated E0277 (like Rust) instead of a
        // generic type mismatch
        if let (Ty::Trait(t), Ty::Struct(sname)) = (expected, found) {
            self.e(terr(
                "E0277",
                format!(
                    "the trait `{}` is not implemented for `{}` (required by {})",
                    t, sname, what
                ),
                span,
            ));
            return;
        }
        self.e(terr(
            "E0308",
            format!(
                "{}: expected {}, found {}",
                what,
                ty_name(expected),
                ty_name(found)
            ),
            span,
        ));
    }
}

fn check_match_exhaustive(ck: &mut Ck, subject_ty: &Ty, arms: &[MatchArm], sp: Span) {
    let irrefutable = arms.iter().any(|a| {
        a.pats
            .iter()
            .any(|p| matches!(p, Pattern::Wildcard | Pattern::Ident(_)))
    });
    if irrefutable {
        return;
    }
    match subject_ty {
        Ty::Bool => {
            let has_true = arms
                .iter()
                .any(|a| a.pats.iter().any(|p| matches!(p, Pattern::Bool(true))));
            let has_false = arms
                .iter()
                .any(|a| a.pats.iter().any(|p| matches!(p, Pattern::Bool(false))));
            if !(has_true && has_false) {
                let missing = match (has_true, has_false) {
                    (true, false) => "false",
                    (false, true) => "true",
                    _ => "true and false",
                };
                ck.e(terr(
                    "E0004",
                    format!("non-exhaustive match on bool: missing {}", missing),
                    sp,
                ));
            }
        }
        Ty::Option(_) => {
            let has_null = arms
                .iter()
                .any(|a| a.pats.iter().any(|p| matches!(p, Pattern::Null)));
            let has_some = arms.iter().any(|a| {
                a.pats
                    .iter()
                    .any(|p| matches!(p, Pattern::Variant(n, _) if n == "Some"))
            });
            if !(has_null && has_some) {
                ck.e(terr(
                    "E0004",
                    "non-exhaustive match on nullable value: expected Some(...) and null/None arms",
                    sp,
                ));
            }
        }
        Ty::Enum(en) => {
            if let Some(meta) = ck.info.enums.get(en) {
                for v in &meta.order {
                    let has = arms.iter().any(|a| {
                        a.pats
                            .iter()
                            .any(|p| matches!(p, Pattern::Variant(n, _) if n == v))
                    });
                    if !has {
                        ck.e(terr(
                            "E0004",
                            format!("non-exhaustive match on {}: missing {}", en, v),
                            sp,
                        ));
                        break;
                    }
                }
            }
        }
        Ty::Result(_, _) => {
            let has_ok = arms.iter().any(|a| {
                a.pats
                    .iter()
                    .any(|p| matches!(p, Pattern::Variant(n, _) if n == "Ok"))
            });
            let has_err = arms.iter().any(|a| {
                a.pats
                    .iter()
                    .any(|p| matches!(p, Pattern::Variant(n, _) if n == "Err"))
            });
            if !(has_ok && has_err) {
                ck.e(terr(
                    "E0004",
                    "non-exhaustive match on Result: expected Ok(...) and Err(...) arms",
                    sp,
                ));
            }
        }
        _ => {}
    }
}

fn bind_pattern_vars(scope: &mut FeScope<'_>, pat: &Pattern, subject_ty: &Ty) {
    match pat {
        Pattern::Ident(n) => scope.declare(n, subject_ty.clone(), VarKind::Auto, false),
        Pattern::Variant(name, args) => {
            let field_ty = match (name.as_str(), subject_ty) {
                ("Some", Ty::Option(inner)) => (**inner).clone(),
                ("Ok", Ty::Result(ok, _)) => (**ok).clone(),
                ("Err", Ty::Result(_, err)) => (**err).clone(),
                _ => Ty::Any,
            };
            for a in args {
                bind_pattern_vars(scope, a, &field_ty);
            }
        }
        _ => {}
    }
}

/// names of builtin conversions with statically known return types
fn builtin_ret_ty(name: &str) -> Option<Ty> {
    Some(match name {
        "int" | "len" | "floor" | "ceil" | "round" => Ty::Int,
        "float" | "sqrt" | "abs_f" => Ty::Float,
        "str" | "type" | "trim" => Ty::Str,
        "bool" => Ty::Bool,
        _ => return None,
    })
}

// ---------------- checker environment per function ----------------

/// mark a node as holding a guardable typed slot (int/float/bool) so the
/// interpreter mirrors the native backend's boundary guards/conversions
fn set_guard_flag<T>(n: &crate::ast::Node<T>, t: &Ty) {
    let bit = match t {
        Ty::Int => crate::ast::FLAG_GUARD_INT,
        Ty::Float => crate::ast::FLAG_GUARD_FLOAT,
        Ty::Bool => crate::ast::FLAG_GUARD_BOOL,
        Ty::Option(inner) => {
            set_guard_flag(n, inner);
            crate::ast::FLAG_GUARD_NULLABLE
        }
        _ => 0,
    };
    if bit != 0 {
        n.flags.set(n.flags.get() | bit);
    }
}

struct FnEnv {
    /// flow types (best knowledge at the point)
    tys: HashMap<String, Ty>,
    /// variable kinds, for const checks on &mut method receivers
    kinds: HashMap<String, VarKind>,
    /// per-variable raw-type candidate (meet over every write)
    finals: HashMap<String, Ty>,
    /// return annotation of the current function
    ret_expected: Option<Ty>,
    fn_name: String,
}

impl Ck {
    /// globals visible inside every function body (top-level fns + struct types)
    fn global_env(&self) -> HashMap<String, Ty> {
        let mut m = HashMap::new();
        for (n, f) in &self.info.top_fns {
            let id = FuncDef::id(f);
            if let Some(sig) = self.info.fn_sigs.get(&id) {
                m.insert(n.clone(), Ty::Func(Rc::new(sig.clone())));
            }
        }
        for n in self.info.structs.keys() {
            m.insert(n.clone(), Ty::TypeVal(n.clone()));
        }
        for (en, em) in &self.info.enums {
            for (vn, fields) in &em.variants {
                if fields.is_empty() {
                    m.insert(vn.clone(), Ty::Enum(en.clone()));
                } else {
                    m.insert(
                        vn.clone(),
                        Ty::Func(Rc::new(FnSig {
                            params: fields.iter().cloned().map(Some).collect(),
                            rest: false,
                            defaults: 0,
                            ret: Some(Ty::Enum(en.clone())),
                        })),
                    );
                }
            }
        }
        m
    }

    fn check_fn(&mut self, def: &Rc<FuncDef>, self_ty: Option<Ty>, sig: Option<FnSig>) {
        let mut fe = FnEnv {
            tys: self.global_env(),
            kinds: HashMap::new(),
            finals: HashMap::new(),
            ret_expected: sig.as_ref().and_then(|s| s.ret.clone()),
            fn_name: def.name.clone(),
        };
        // seed parameters
        let mut params_seed: Vec<(String, Ty)> = Vec::new();
        let mut pi = 0usize;
        for p in &def.params {
            let t = if p.name == "self" && def.receiver.is_some() {
                self_ty.clone().unwrap_or(Ty::Any)
            } else if p.rest {
                Ty::Arr(Box::new(Ty::Any))
            } else {
                sig.as_ref()
                    .and_then(|s| s.params.get(pi).cloned())
                    .flatten()
                    .unwrap_or(Ty::Any)
            };
            if !(p.name == "self" && def.receiver.is_some()) && !p.rest {
                pi += 1;
            }
            params_seed.push((p.name.clone(), t.clone()));
            fe.finals.insert(p.name.clone(), t);
            fe.kinds.insert(p.name.clone(), VarKind::Auto);
        }
        for (n, t) in &params_seed {
            fe.tys.insert(n.clone(), t.clone());
        }
        // walk the body twice; the second pass is silent and exists so that
        // loop-carried demotions propagate (a raw candidate assigned a
        // dynamic value inside a while loop must demote everywhere)
        let seed_env = fe.tys.clone();
        {
            let mut env = FeScope { fe: &mut fe };
            env.stmts(self, &def.body, 0);
        }
        self.silent = true;
        fe.tys = seed_env;
        {
            let mut env = FeScope { fe: &mut fe };
            env.stmts(self, &def.body, 0);
        }
        self.silent = false;
        // record raw-able locals (only provable scalars; never `self`)
        let id = FuncDef::id(def);
        let mut m = HashMap::new();
        for (n, t) in &fe.finals {
            if matches!(t, Ty::Int | Ty::Float | Ty::Bool) && n != "self" {
                m.insert(n.clone(), t.clone());
            }
        }
        for p in &def.params {
            if let Some(t) = fe.finals.get(&p.name) {
                if matches!(t, Ty::Int | Ty::Float | Ty::Bool) && p.name != "self" {
                    m.insert(p.name.clone(), t.clone());
                }
            }
        }
        self.info.local_tys.insert(id, m);
    }

    fn check_toplevel(&mut self, stmts: &[Stmt]) {
        let mut fe = FnEnv {
            tys: self.global_env(),
            kinds: HashMap::new(),
            finals: HashMap::new(),
            ret_expected: None,
            fn_name: "<main>".into(),
        };
        let mut env = FeScope { fe: &mut fe };
        env.stmts(self, stmts, 0);
    }
}

/// statement walker holding a per-function environment
struct FeScope<'a> {
    fe: &'a mut FnEnv,
}

impl<'a> FeScope<'a> {
    fn declare(&mut self, name: &str, ty: Ty, kind: VarKind, is_param_def: bool) {
        self.fe.tys.insert(name.to_string(), ty.clone());
        self.fe.kinds.insert(name.to_string(), kind);
        if is_param_def {
            self.fe.finals.insert(name.to_string(), ty);
        } else {
            // re-declaration: the raw candidate narrows to the meet
            match self.fe.finals.get(name) {
                None => {
                    self.fe.finals.insert(name.to_string(), ty);
                }
                Some(prev) => {
                    let m = meet(prev, &ty);
                    self.fe.finals.insert(name.to_string(), m);
                }
            }
        }
    }
    fn assign_ty(&mut self, name: &str, ty: Ty) {
        self.fe.tys.insert(name.to_string(), ty.clone());
        match self.fe.finals.get(name) {
            None => {
                self.fe.finals.insert(name.to_string(), ty);
            }
            Some(prev) => {
                let m = meet(prev, &ty);
                self.fe.finals.insert(name.to_string(), m);
            }
        }
    }

    fn stmts(&mut self, ck: &mut Ck, stmts: &[Stmt], depth: usize) {
        for s in stmts {
            self.stmt(ck, s, depth);
        }
    }

    fn stmt(&mut self, ck: &mut Ck, s: &Stmt, depth: usize) {
        match &s.node {
            StmtKind::Var {
                kind,
                name,
                value,
                ty,
            } => {
                let vty = self.expr(ck, value);
                let declared = ty.as_ref().map(|t| ck.lower_ty(t));
                if let Some(dt) = &declared {
                    ck.must_assign(dt, &vty, s.span, &format!("variable \"{}\"", name));
                    self.declare(name, dt.clone(), *kind, false);
                    set_guard_flag(s, dt);
                } else {
                    // trust the flow type only if the value type is concrete
                    let dt = if matches!(vty, Ty::Any) { Ty::Any } else { vty };
                    self.declare(name, dt, *kind, false);
                }
            }
            StmtKind::Func(_) => {} // bodies checked separately
            StmtKind::Enum { .. } => {}
            StmtKind::Struct { name, fields } => {
                // check default values against declared field types
                let metas: Vec<(String, Ty, Option<Expr>, Span)> = {
                    let sm = match ck.info.structs.get(name) {
                        Some(x) => x,
                        None => return,
                    };
                    fields
                        .iter()
                        .map(|f| {
                            (
                                f.name.clone(),
                                sm.fields
                                    .get(&f.name)
                                    .map(|m| m.ty.clone())
                                    .unwrap_or(Ty::Any),
                                f.default.clone(),
                                f.span,
                            )
                        })
                        .collect()
                };
                for (fname, fty, dflt, fsp) in metas {
                    if let Some(de) = dflt {
                        let dt = self.expr(ck, &de);
                        ck.must_assign(
                            &fty,
                            &dt,
                            fsp,
                            &format!("default value of field \"{}\"", fname),
                        );
                    }
                }
            }
            StmtKind::Impl { .. } | StmtKind::Trait { .. } | StmtKind::Import { .. } => {}
            StmtKind::ExprStmt(e) => {
                self.expr(ck, e);
            }
            StmtKind::Block(b) => self.stmts(ck, b, depth + 1),
            StmtKind::If { cond, then, els } => {
                self.expr(ck, cond);
                // branch: run both sides from the same env, merge
                let saved = (self.fe.tys.clone(), self.fe.finals.clone());
                self.stmt(ck, then, depth);
                let after_then = (self.fe.tys.clone(), self.fe.finals.clone());
                self.fe.tys = saved.0.clone();
                self.fe.finals = saved.1.clone();
                if let Some(e) = els {
                    self.stmt(ck, e, depth);
                }
                let after_els = (self.fe.tys.clone(), self.fe.finals.clone());
                self.fe.tys = merge_envs(&after_then.0, &after_els.0, &saved.0);
                self.fe.finals = merge_envs(&after_then.1, &after_els.1, &saved.1);
            }
            StmtKind::While { cond, body } => {
                self.expr(ck, cond);
                let saved = (self.fe.tys.clone(), self.fe.finals.clone());
                self.stmt(ck, body, depth + 1);
                let after = (self.fe.tys.clone(), self.fe.finals.clone());
                self.fe.tys = merge_envs(&after.0, &saved.0, &saved.0);
                self.fe.finals = merge_envs(&after.1, &saved.1, &saved.1);
            }
            StmtKind::ForC {
                init,
                cond,
                step,
                body,
            } => {
                if let Some(i) = init {
                    self.stmt(ck, i, depth + 1);
                }
                let saved = (self.fe.tys.clone(), self.fe.finals.clone());
                if let Some(c) = cond {
                    self.expr(ck, c);
                }
                self.stmt(ck, body, depth + 1);
                if let Some(st) = step {
                    self.expr(ck, st);
                }
                let after = (self.fe.tys.clone(), self.fe.finals.clone());
                self.fe.tys = merge_envs(&after.0, &saved.0, &saved.0);
                self.fe.finals = merge_envs(&after.1, &saved.1, &saved.1);
            }
            StmtKind::ForIn {
                name,
                iter,
                body,
                ty,
            } => {
                let it_ty = self.expr(ck, iter);
                let elem = match &it_ty {
                    Ty::Arr(e) => (**e).clone(),
                    Ty::Str => Ty::Str,
                    Ty::Map(_, _) | Ty::Any => Ty::Any,
                    other => {
                        ck.e(terr(
                            "E0277",
                            format!("cannot iterate over {}", ty_name(other)),
                            iter.span,
                        ));
                        Ty::Any
                    }
                };
                if let Some(t) = ty {
                    let dt = ck.lower_ty(t);
                    ck.must_assign(&dt, &elem, s.span, &format!("loop variable \"{}\"", name));
                    set_guard_flag(s, &dt);
                    self.declare(name, dt, VarKind::Auto, false);
                } else {
                    self.declare(name, elem, VarKind::Auto, false);
                }
                let saved = (self.fe.tys.clone(), self.fe.finals.clone());
                self.stmt(ck, body, depth + 1);
                let after = (self.fe.tys.clone(), self.fe.finals.clone());
                self.fe.tys = merge_envs(&after.0, &saved.0, &saved.0);
                self.fe.finals = merge_envs(&after.1, &saved.1, &saved.1);
            }
            StmtKind::MatchStmt { subject, arms } => {
                let st = self.expr(ck, subject);
                check_match_exhaustive(ck, &st, arms, subject.span);
                let saved0 = (self.fe.tys.clone(), self.fe.finals.clone());
                let mut acc: Option<(HashMap<String, Ty>, HashMap<String, Ty>)> = None;
                for a in arms {
                    self.fe.tys = saved0.0.clone();
                    self.fe.finals = saved0.1.clone();
                    for p in &a.pats {
                        bind_pattern_vars(self, p, &st);
                    }
                    match &a.body {
                        MatchBody::Expr(e) => {
                            self.expr(ck, e);
                        }
                        MatchBody::Block(b) => self.stmts(ck, b, depth + 1),
                    }
                    let cur = (self.fe.tys.clone(), self.fe.finals.clone());
                    acc = Some(match acc {
                        None => cur,
                        Some(prev) => (
                            merge_envs(&prev.0, &cur.0, &saved0.0),
                            merge_envs(&prev.1, &cur.1, &saved0.1),
                        ),
                    });
                }
                if let Some((t1, t2)) = acc {
                    self.fe.tys = t1;
                    self.fe.finals = t2;
                }
            }
            StmtKind::Return(v) => {
                let rt = match v {
                    Some(e) => self.expr(ck, e),
                    None => Ty::Null,
                };
                if let Some(exp) = &self.fe.ret_expected {
                    let what = format!("return value of {}", self.fe.fn_name);
                    ck.must_assign(exp, &rt, s.span, &what);
                }
            }
            StmtKind::Break | StmtKind::Continue => {}
        }
    }

    // ---------------- expressions ----------------
    fn expr(&mut self, ck: &mut Ck, e: &Expr) -> Ty {
        match &e.node {
            ExprKind::Null => Ty::Null,
            ExprKind::Bool(_) => Ty::Bool,
            ExprKind::Int(_) => Ty::Int,
            ExprKind::Float(_) => Ty::Float,
            ExprKind::Str(_) => Ty::Str,
            ExprKind::Ident(name) => self.fe.tys.get(name).cloned().unwrap_or(Ty::Any),
            ExprKind::Array(items) => {
                let mut el = Ty::Any;
                let mut known = false;
                for it in items {
                    let t = self.expr(ck, it);
                    el = if !known {
                        known = true;
                        t
                    } else {
                        join(&el, &t)
                    };
                }
                if items.is_empty() {
                    Ty::Arr(Box::new(Ty::Any))
                } else {
                    Ty::Arr(Box::new(el))
                }
            }
            ExprKind::Object(ps) => {
                let mut vt = Ty::Any;
                let mut known = false;
                for (_, ve) in ps {
                    let t = self.expr(ck, ve);
                    vt = if !known {
                        known = true;
                        t
                    } else {
                        join(&vt, &t)
                    };
                }
                Ty::Map(Box::new(Ty::Str), Box::new(vt))
            }
            ExprKind::Unary(op, x) => {
                let t = self.expr(ck, x);
                match op {
                    UnOp::Not => Ty::Bool,
                    UnOp::Neg => match t {
                        Ty::Int => Ty::Int,
                        Ty::Float => Ty::Float,
                        Ty::Any => Ty::Any,
                        other => {
                            ck.e(terr(
                                "E0308",
                                format!("cannot apply unary - to {}", ty_name(&other)),
                                e.span,
                            ));
                            Ty::Any
                        }
                    },
                    UnOp::BitNot => match t {
                        Ty::Int => Ty::Int,
                        Ty::Any => Ty::Any,
                        other => {
                            ck.e(terr(
                                "E0308",
                                format!("cannot apply ~ to {}", ty_name(&other)),
                                e.span,
                            ));
                            Ty::Any
                        }
                    },
                }
            }
            ExprKind::Borrow { expr, .. } => self.expr(ck, expr),
            ExprKind::Binary(op, a, b) => {
                let ta = self.expr(ck, a);
                let tb = self.expr(ck, b);
                let r = self.binary_ty(ck, *op, &ta, &tb, e.span);
                // provably int-only arithmetic: strict-overflow zone
                if matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul)
                    && ta == Ty::Int
                    && tb == Ty::Int
                    && r == Ty::Int
                {
                    e.flags
                        .set(e.flags.get() | crate::ast::FLAG_STRICT_INT_ARITH);
                }
                r
            }
            ExprKind::Logical(_, a, b) => {
                let ta = self.expr(ck, a);
                let tb = self.expr(ck, b);
                join(&ta, &tb)
            }
            ExprKind::Ternary(c, a, b) => {
                self.expr(ck, c);
                let ta = self.expr(ck, a);
                let tb = self.expr(ck, b);
                join(&ta, &tb)
            }
            ExprKind::Assign { target, op, value } => {
                let vt = self.expr(ck, value);
                // typed slot (annotated local): the interpreter must apply
                // the same boundary guard/conversion the native slot would
                if let AssignTarget::Ident(n) = target {
                    if let Some(lt) = self.fe.tys.get(n).cloned() {
                        set_guard_flag(e, &lt);
                    }
                }
                // compound assignment re-checks like the matching binop
                let result_ty = if *op == AssignOp::Eq {
                    vt.clone()
                } else {
                    let lt = self.target_ty(ck, target);
                    let bop = match op {
                        AssignOp::Add => BinOp::Add,
                        AssignOp::Sub => BinOp::Sub,
                        AssignOp::Mul => BinOp::Mul,
                        AssignOp::Div => BinOp::Div,
                        AssignOp::Mod => BinOp::Mod,
                        AssignOp::Eq => unreachable!(),
                    };
                    let r = self.binary_ty(ck, bop, &lt, &vt, e.span);
                    if matches!(op, AssignOp::Add | AssignOp::Sub | AssignOp::Mul)
                        && lt == Ty::Int
                        && r == Ty::Int
                    {
                        e.flags
                            .set(e.flags.get() | crate::ast::FLAG_STRICT_INT_ARITH);
                    }
                    r
                };
                self.check_target_write(ck, target, &result_ty, e.span);
                result_ty
            }
            ExprKind::Call(callee, args) => self.call_ty(ck, callee, args, e.span),
            ExprKind::Index(obj, idx) => {
                let ot = self.expr(ck, obj);
                let it = self.expr(ck, idx);
                match &ot {
                    Ty::Arr(el) => {
                        if !matches!(it, Ty::Int | Ty::Any) {
                            ck.e(terr(
                                "E0308",
                                format!("array index must be int, found {}", ty_name(&it)),
                                idx.span,
                            ));
                        }
                        (**el).clone()
                    }
                    Ty::Str => Ty::Str,
                    Ty::Map(k, v) => {
                        let _ = k;
                        (**v).clone()
                    }
                    Ty::Any => Ty::Any,
                    other => {
                        ck.e(terr(
                            "E0277",
                            format!("cannot index {}", ty_name(other)),
                            e.span,
                        ));
                        Ty::Any
                    }
                }
            }
            ExprKind::Slice { obj, start, end } => {
                let ot = self.expr(ck, obj);
                if let Some(s) = start {
                    let t = self.expr(ck, s);
                    if !matches!(t, Ty::Int | Ty::Any) {
                        ck.e(terr(
                            "E0308",
                            format!("slice bound must be int, found {}", ty_name(&t)),
                            s.span,
                        ));
                    }
                }
                if let Some(en) = end {
                    self.expr(ck, en);
                }
                match &ot {
                    Ty::Arr(_) | Ty::Str => ot,
                    Ty::Any => Ty::Any,
                    other => {
                        ck.e(terr(
                            "E0277",
                            format!("cannot slice {}", ty_name(other)),
                            e.span,
                        ));
                        Ty::Any
                    }
                }
            }
            ExprKind::Member(obj, name) => self.member_ty(ck, obj, name, e.span),
            ExprKind::FuncLit(def) => {
                let id = FuncDef::id(def);
                match ck.info.fn_sigs.get(&id) {
                    Some(sig) => Ty::Func(Rc::new(sig.clone())),
                    None => Ty::Func(Rc::new(FnSig::default())),
                }
            }
            ExprKind::Match { subject, arms } => {
                let st = self.expr(ck, subject);
                check_match_exhaustive(ck, &st, arms, subject.span);
                let saved = self.fe.tys.clone();
                let mut acc: Option<Ty> = None;
                for a in arms {
                    self.fe.tys = saved.clone();
                    for p in &a.pats {
                        bind_pattern_vars(self, p, &st);
                    }
                    let at = match &a.body {
                        MatchBody::Expr(e2) => self.expr(ck, e2),
                        MatchBody::Block(b) => {
                            self.stmts(ck, b, 1);
                            Ty::Null
                        }
                    };
                    acc = Some(match acc {
                        None => at,
                        Some(prev) => join(&prev, &at),
                    });
                }
                self.fe.tys = saved;
                acc.unwrap_or(Ty::Any)
            }
            ExprKind::StructLit { name, fields } => self.struct_lit_ty(ck, name, fields, e.span),
        }
    }

    fn target_ty(&mut self, ck: &mut Ck, t: &AssignTarget) -> Ty {
        match t {
            AssignTarget::Ident(n) => self.fe.tys.get(n).cloned().unwrap_or(Ty::Any),
            AssignTarget::Index(o, i) => {
                let e2 = Expr::new(ExprKind::Index(o.clone(), i.clone()), o.span);
                self.expr(ck, &e2)
            }
            AssignTarget::Member(o, m) => {
                let e2 = Expr::new(ExprKind::Member(o.clone(), m.clone()), o.span);
                self.expr(ck, &e2)
            }
        }
    }

    fn check_target_write(&mut self, ck: &mut Ck, t: &AssignTarget, vt: &Ty, sp: Span) {
        match t {
            AssignTarget::Ident(n) => {
                let cur = self.fe.tys.get(n).cloned();
                if let Some(c) = &cur {
                    if !matches!(c, Ty::Any) {
                        ck.must_assign(c, vt, sp, &format!("assignment to \"{}\"", n));
                    }
                }
                let newt = match &cur {
                    Some(c) if !matches!(c, Ty::Any) => c.clone(),
                    _ => vt.clone(),
                };
                self.assign_ty(n, newt);
            }
            AssignTarget::Index(oe, ie) => {
                let ot = self.expr(ck, oe);
                self.expr(ck, ie);
                match &ot {
                    Ty::Arr(el) => {
                        ck.must_assign(el, vt, sp, "array element assignment");
                    }
                    Ty::Map(_, mv) => {
                        ck.must_assign(mv, vt, sp, "map value assignment");
                    }
                    Ty::Any => {}
                    other => {
                        ck.e(terr(
                            "E0277",
                            format!("cannot index-assign {}", ty_name(other)),
                            sp,
                        ));
                    }
                }
            }
            AssignTarget::Member(oe, name) => {
                let ot = self.expr(ck, oe);
                match &ot {
                    Ty::Struct(sn) => {
                        let fty = ck
                            .info
                            .structs
                            .get(sn)
                            .and_then(|sm| sm.fields.get(name))
                            .map(|f| f.ty.clone());
                        match fty {
                            Some(ft) => {
                                ck.must_assign(&ft, vt, sp, &format!("field \"{}\"", name));
                            }
                            None => {
                                if ck
                                    .info
                                    .structs
                                    .get(sn)
                                    .map(|s| s.methods.contains_key(name))
                                    .unwrap_or(false)
                                {
                                    ck.e(terr(
                                        "E0308",
                                        format!("cannot assign to method \"{}\" of {}", name, sn),
                                        sp,
                                    ));
                                } else {
                                    ck.e(terr(
                                        "E0609",
                                        format!("no field \"{}\" on type {}", name, sn),
                                        sp,
                                    ));
                                }
                            }
                        }
                    }
                    Ty::Map(_, mv) => {
                        ck.must_assign(mv, vt, sp, "member assignment");
                    }
                    Ty::Any => {}
                    other => {
                        ck.e(terr(
                            "E0308",
                            format!("cannot set member \"{}\" on {}", name, ty_name(other)),
                            sp,
                        ));
                    }
                }
            }
        }
    }

    fn member_ty(&mut self, ck: &mut Ck, obj: &Expr, name: &str, sp: Span) -> Ty {
        let ot = self.expr(ck, obj);
        match &ot {
            Ty::Any => Ty::Any,
            Ty::Map(_, v) => (**v).clone(),
            Ty::Option(inner) => {
                ck.e(terr(
                    "E0308",
                    format!(
                        "cannot access member \"{}\" on nullable {} without checking for null",
                        name,
                        ty_name(inner)
                    ),
                    sp,
                ));
                Ty::Any
            }
            Ty::TypeVal(real) => {
                // the struct TYPE value itself: associated functions (new, …)
                let sm = match ck.info.structs.get(real) {
                    Some(x) => x,
                    None => return Ty::Any,
                };
                match sm.methods.get(name) {
                    Some(mm) => Ty::Func(Rc::new(mm.sig.clone())),
                    None => {
                        ck.e(terr(
                            "E0599",
                            format!(
                                "no associated item named \"{}\" found for struct {}",
                                name, real
                            ),
                            sp,
                        ));
                        Ty::Any
                    }
                }
            }
            Ty::Struct(sn) => {
                let sm = match ck.info.structs.get(sn) {
                    Some(x) => x,
                    None => return Ty::Any,
                };
                if let Some(f) = sm.fields.get(name) {
                    return f.ty.clone();
                }
                if let Some(mm) = sm.methods.get(name) {
                    return Ty::Func(Rc::new(mm.sig.clone()));
                }
                // unambiguous trait method?
                let mut hits = 0;
                let mut last: Option<Rc<FnSig>> = None;
                for (tname, tbl) in &sm.trait_impls {
                    if let Some(d) = tbl.get(name) {
                        hits += 1;
                        let _ = tname;
                        let id = FuncDef::id(d);
                        last = ck.info.fn_sigs.get(&id).map(|s| Rc::new(s.clone()));
                    }
                }
                match hits {
                    0 => {
                        ck.e(terr(
                            "E0599",
                            format!("no field or method \"{}\" found on struct {}", name, sn),
                            sp,
                        ));
                        Ty::Any
                    }
                    1 => Ty::Func(last.unwrap_or_else(|| Rc::new(FnSig::default()))),
                    _ => {
                        ck.e(terr(
                            "E0034",
                            format!("multiple applicable items in scope for method \"{}\"", name),
                            sp,
                        ));
                        Ty::Any
                    }
                }
            }
            Ty::Trait(tn) => {
                let tm = match ck.info.traits.get(tn) {
                    Some(x) => x,
                    None => return Ty::Any,
                };
                match tm.methods.get(name) {
                    Some(mm) => Ty::Func(Rc::new(mm.sig.clone())),
                    None => {
                        ck.e(terr(
                            "E0599",
                            format!("no method \"{}\" found on trait {}", name, tn),
                            sp,
                        ));
                        Ty::Any
                    }
                }
            }
            Ty::Arr(_) | Ty::Str => {
                ck.e(terr(
                    "E0599",
                    format!(
                        "no member \"{}\" on {} (use functions like len, map, split)",
                        name,
                        ty_name(&ot)
                    ),
                    sp,
                ));
                Ty::Any
            }
            other => {
                ck.e(terr(
                    "E0599",
                    format!("no member \"{}\" on {}", name, ty_name(other)),
                    sp,
                ));
                Ty::Any
            }
        }
    }

    fn call_ty(&mut self, ck: &mut Ck, callee: &Expr, args: &[Expr], sp: Span) -> Ty {
        // Built-in generic constructors. Option is represented as `T | null`:
        // `Some(x)` is just `x` at runtime, while `None` lexes as `null`.
        if let ExprKind::Ident(n) = &callee.node {
            match n.as_str() {
                "Some" => {
                    if args.len() != 1 {
                        ck.e(terr("E0061", "Some takes exactly 1 argument", sp));
                        return Ty::Option(Box::new(Ty::Any));
                    }
                    let t = self.expr(ck, &args[0]);
                    return Ty::Option(Box::new(t));
                }
                "Ok" => {
                    if args.len() != 1 {
                        ck.e(terr("E0061", "Ok takes exactly 1 argument", sp));
                        return Ty::Result(Box::new(Ty::Any), Box::new(Ty::Any));
                    }
                    let t = self.expr(ck, &args[0]);
                    return Ty::Result(Box::new(t), Box::new(Ty::Any));
                }
                "Err" => {
                    if args.len() != 1 {
                        ck.e(terr("E0061", "Err takes exactly 1 argument", sp));
                        return Ty::Result(Box::new(Ty::Any), Box::new(Ty::Any));
                    }
                    let t = self.expr(ck, &args[0]);
                    return Ty::Result(Box::new(Ty::Any), Box::new(t));
                }
                _ => {}
            }
        }

        // method call on a &mut receiver bound to a const?
        if let ExprKind::Member(obj, name) = &callee.node {
            // find the method meta for the receiver's static type
            let ot = self.expr(ck, obj);
            let meta: Option<(bool, String)> = match &ot {
                Ty::Struct(sn) => ck
                    .info
                    .structs
                    .get(sn)
                    .and_then(|sm| sm.methods.get(name))
                    .map(|mm| (mm.mutable, sn.clone())),
                _ => None,
            };
            if let Some((mutable, sn)) = meta {
                if mutable {
                    if let ExprKind::Ident(recv_name) = &obj.node {
                        if matches!(self.fe.kinds.get(recv_name), Some(VarKind::Const)) {
                            ck.e(terr(
                                "E0594",
                                format!(
                                    "cannot call &mut self method \"{}\" of {} on const \"{}\"",
                                    name, sn, recv_name
                                ),
                                sp,
                            ));
                        }
                    }
                }
            }
            // fall through: the callee's Func sig does the arg checking
        }

        // callee type
        let ct = match &callee.node {
            ExprKind::Ident(n) => {
                // unit-level function takes priority over flow env
                if let Some(t) = self.fe.tys.get(n) {
                    t.clone()
                } else if let Some(f) = ck.info.top_fns.get(n) {
                    let id = FuncDef::id(f);
                    Ty::Func(Rc::new(
                        ck.info.fn_sigs.get(&id).cloned().unwrap_or_default(),
                    ))
                } else if let Some(rt) = builtin_ret_ty(n) {
                    // known builtin conversion; args unchecked
                    for a in args {
                        self.expr(ck, a);
                    }
                    return rt;
                } else {
                    Ty::Any
                }
            }
            _ => self.expr(ck, callee),
        };

        // calling the struct type itself dispatches to `new`
        let ct = match &ct {
            Ty::TypeVal(real) => {
                match ck
                    .info
                    .structs
                    .get(real)
                    .and_then(|sm| sm.methods.get("new"))
                {
                    Some(mm) => Ty::Func(Rc::new(mm.sig.clone())),
                    None => {
                        ck.e(terr(
                            "E0618",
                            format!("struct {} is not callable (use {} {{ .. }})", real, real),
                            sp,
                        ));
                        Ty::Any
                    }
                }
            }
            other => other.clone(),
        };

        match ct {
            Ty::Func(sig) => {
                // collect arg types
                let mut ats = Vec::with_capacity(args.len());
                for a in args {
                    ats.push(self.expr(ck, a));
                }
                let min = sig.min_args();
                if !sig.rest && ats.len() > sig.params.len() {
                    ck.e(terr(
                        "E0061",
                        format!(
                            "this function takes at most {} argument(s) but {} were supplied",
                            sig.params.len(),
                            ats.len()
                        ),
                        sp,
                    ));
                }
                if ats.len() < min {
                    ck.e(terr(
                        "E0061",
                        format!(
                            "this function takes at least {} argument(s) but {} were supplied",
                            min,
                            ats.len()
                        ),
                        sp,
                    ));
                }
                for (i, at) in ats.iter().enumerate() {
                    if let Some(Some(pt)) = sig.params.get(i) {
                        ck.must_assign(pt, at, args[i].span, &format!("argument {}", i + 1));
                    }
                }
                sig.ret.clone().unwrap_or(Ty::Any)
            }
            Ty::Any => {
                for a in args {
                    self.expr(ck, a);
                }
                Ty::Any
            }
            other => {
                for a in args {
                    self.expr(ck, a);
                }
                ck.e(terr(
                    "E0618",
                    format!("expected function, found {}", ty_name(&other)),
                    callee.span,
                ));
                Ty::Any
            }
        }
    }

    fn struct_lit_ty(
        &mut self,
        ck: &mut Ck,
        name: &str,
        fields: &[(String, Expr)],
        sp: Span,
    ) -> Ty {
        if !ck.info.structs.contains_key(name) {
            ck.e(terr(
                "E0422",
                format!(
                    "cannot find struct \"{}\" in this scope (struct literal)",
                    name
                ),
                sp,
            ));
            for (_, ve) in fields {
                self.expr(ck, ve);
            }
            return Ty::Any;
        }
        let (field_tys, required): (HashMap<String, Ty>, Vec<String>) = {
            let sm = &ck.info.structs[name];
            let mut req = Vec::new();
            let mut tys = HashMap::new();
            for fnm in &sm.field_order {
                let m = &sm.fields[fnm];
                tys.insert(fnm.clone(), m.ty.clone());
                if !m.has_default {
                    req.push(fnm.clone());
                }
            }
            (tys, req)
        };
        let mut seen: HashMap<String, Span> = HashMap::new();
        for (fname, ve) in fields {
            let vt = self.expr(ck, ve);
            if let Some(prev) = seen.get(fname) {
                let mut er = terr(
                    "E0625",
                    format!("field \"{}\" specified more than once", fname),
                    ve.span,
                );
                er.notes.push(("first used here".into(), *prev));
                ck.e(er);
                continue;
            }
            seen.insert(fname.clone(), ve.span);
            match field_tys.get(fname) {
                Some(ft) => {
                    ck.must_assign(ft, &vt, ve.span, &format!("field \"{}\"", fname));
                }
                None => {
                    ck.e(terr(
                        "E0609",
                        format!("struct {} has no field named \"{}\"", name, fname),
                        ve.span,
                    ));
                }
            }
        }
        for r in &required {
            if !seen.contains_key(r) {
                ck.e(terr(
                    "E0063",
                    format!("missing field \"{}\" in initializer of {}", r, name),
                    sp,
                ));
            }
        }
        Ty::Struct(name.to_string())
    }

    fn binary_ty(&mut self, ck: &mut Ck, op: BinOp, ta: &Ty, tb: &Ty, sp: Span) -> Ty {
        use BinOp::*;
        let some_any = matches!(ta, Ty::Any) || matches!(tb, Ty::Any);
        let num = |t: &Ty| matches!(t, Ty::Int | Ty::Float);
        match op {
            Add => {
                if some_any {
                    return Ty::Any;
                }
                match (ta, tb) {
                    (Ty::Int, Ty::Int) => Ty::Int,
                    (a, b) if num(a) && num(b) => Ty::Float,
                    (Ty::Str, Ty::Str) => Ty::Str,
                    (Ty::Arr(_), Ty::Arr(_)) => Ty::Arr(Box::new(Ty::Any)),
                    _ => {
                        ck.e(terr(
                            "E0308",
                            format!("cannot add {} and {}", ty_name(ta), ty_name(tb)),
                            sp,
                        ));
                        Ty::Any
                    }
                }
            }
            Sub | Mul | Mod => {
                if some_any {
                    return Ty::Any;
                }
                match (ta, tb) {
                    (Ty::Int, Ty::Int) => Ty::Int,
                    (a, b) if num(a) && num(b) => Ty::Float,
                    (Ty::Str, Ty::Int) if matches!(op, Mul) => Ty::Str,
                    _ => {
                        ck.e(terr(
                            "E0308",
                            format!(
                                "cannot apply {:?} to {} and {}",
                                op,
                                ty_name(ta),
                                ty_name(tb)
                            ),
                            sp,
                        ));
                        Ty::Any
                    }
                }
            }
            Div => {
                if some_any {
                    return Ty::Any;
                }
                if num(ta) && num(tb) {
                    Ty::Float
                } else {
                    ck.e(terr(
                        "E0308",
                        format!("cannot divide {} by {}", ty_name(ta), ty_name(tb)),
                        sp,
                    ));
                    Ty::Any
                }
            }
            Eq | Ne => Ty::Bool,
            Lt | Le | Gt | Ge => {
                if some_any {
                    return Ty::Bool;
                }
                let ok = (num(ta) && num(tb)) || matches!((ta, tb), (Ty::Str, Ty::Str));
                if !ok {
                    ck.e(terr(
                        "E0308",
                        format!("cannot compare {} and {}", ty_name(ta), ty_name(tb)),
                        sp,
                    ));
                }
                Ty::Bool
            }
            BAnd | BOr | BXor | Shl | Shr => {
                if some_any {
                    return Ty::Any;
                }
                if matches!(ta, Ty::Int) && matches!(tb, Ty::Int) {
                    Ty::Int
                } else {
                    ck.e(terr(
                        "E0308",
                        format!(
                            "bitwise {:?} needs ints, found {} and {}",
                            op,
                            ty_name(ta),
                            ty_name(tb)
                        ),
                        sp,
                    ));
                    Ty::Any
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// lattice helpers
// ---------------------------------------------------------------------------

/// flow join (for expression/branch types): Any absorbs everything
fn join(a: &Ty, b: &Ty) -> Ty {
    match (a, b) {
        (Ty::Any, _) | (_, Ty::Any) => Ty::Any,
        (x, y) if x == y => x.clone(),
        (Ty::Int, Ty::Float) | (Ty::Float, Ty::Int) => Ty::Float,
        (Ty::Arr(x), Ty::Arr(y)) => Ty::Arr(Box::new(join(x, y))),
        (Ty::Map(k1, v1), Ty::Map(k2, v2)) => {
            Ty::Map(Box::new(join(k1, k2)), Box::new(join(v1, v2)))
        }
        (Ty::Option(x), Ty::Null) | (Ty::Null, Ty::Option(x)) => Ty::Option(x.clone()),
        (Ty::Null, x) | (x, Ty::Null) => Ty::Option(Box::new(x.clone())),
        (Ty::Option(x), y) | (y, Ty::Option(x)) => Ty::Option(Box::new(join(x, y))),
        _ => Ty::Any,
    }
}

/// storage meet (raw candidate across every write): a slot is raw-able only
/// if EVERY value ever written to it has the same provable scalar type.
/// `Any` absorbs (an untyped write poisons the slot forever); int widens
/// once to float.
fn meet(prev: &Ty, new: &Ty) -> Ty {
    match (prev, new) {
        (x, y) if x == y => x.clone(),
        (Ty::Int, Ty::Float) | (Ty::Float, Ty::Int) => Ty::Float,
        (Ty::Option(x), Ty::Option(y)) => Ty::Option(Box::new(meet(x, y))),
        (Ty::Option(x), Ty::Null) | (Ty::Null, Ty::Option(x)) => Ty::Option(x.clone()),
        (Ty::Option(x), y) | (y, Ty::Option(x)) => Ty::Option(Box::new(meet(x, y))),
        _ => Ty::Any,
    }
}

fn merge_envs(
    a: &HashMap<String, Ty>,
    b: &HashMap<String, Ty>,
    before: &HashMap<String, Ty>,
) -> HashMap<String, Ty> {
    let mut out = a.clone();
    for (k, v) in b {
        match out.get(k) {
            Some(cur) => {
                let j = join(cur, v);
                out.insert(k.clone(), j);
            }
            None => {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    // anything deleted in one branch keeps its previous type (flow) —
    // the storage lattice lives in `finals`, which we also merge via join
    for (k, v) in before {
        if !out.contains_key(k) {
            out.insert(k.clone(), v.clone());
        }
    }
    out
}
