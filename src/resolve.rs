//! Static name resolution for the native compiler.
//!
//! For every function in the program computes:
//!   - `captures`: free variables resolving to an *enclosing function's*
//!     locals (globals are never captured). Closure creation passes these
//!     Cells to the child in this exact order;
//!   - `cell_vars`: locals captured by some nested function — allocated as
//!     heap Cells instead of plain registers;
//!   - `locals`: the flat local namespace (params + vars, declaration order).
//!
//! Also assigns global slot indices (builtins first, then user globals).
//!
//! Note (native mode): function bodies are a *flat* namespace — redeclaring
//! a name inside a function is rejected (blocks are not namespaces there).
//! This matches how programs behave under the interpreter's envs.

use crate::ast::*;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

#[derive(Clone)]
pub struct FnRes {
    pub captures: Vec<String>,
    pub cell_vars: HashSet<String>,
    pub locals: Vec<String>,
}

pub struct Resolution {
    pub fns: HashMap<usize, FnRes>,
    pub globals: HashMap<String, usize>,
    pub user_globals: Vec<String>,
}

/// Synthetic function id for a unit's pseudo-function (top-level statements
/// are compiled as plix_main / plix_mod_init_i with this resolution entry).
pub const MAIN_RES_ID: usize = 0;

pub struct ResErr {
    pub msg: String,
    pub line: u32,
    pub col: u32,
}

// ---------------------------------------------------------------------------
// collect every function in the program (decls + literals), recursively
// ---------------------------------------------------------------------------

fn collect_functions(stmts: &[Stmt], out: &mut Vec<Rc<FuncDef>>) {
    for s in stmts {
        collect_in_stmt(s, out);
    }
}

fn collect_in_stmt(s: &Stmt, out: &mut Vec<Rc<FuncDef>>) {
    match &s.node {
        StmtKind::Func(f) => {
            out.push(f.clone());
            collect_functions(&f.body, out);
        }
        StmtKind::Struct { fields, .. } => {
            for f in fields {
                if let Some(d) = &f.default {
                    collect_in_expr(d, out);
                }
            }
        }
        StmtKind::Impl { methods, .. } | StmtKind::Trait { methods, .. } => {
            for m in methods {
                out.push(m.clone());
                collect_functions(&m.body, out);
            }
        }
        StmtKind::Var { value, .. } => collect_in_expr(value, out),
        StmtKind::ExprStmt(e) => collect_in_expr(e, out),
        StmtKind::Block(b) => collect_functions(b, out),
        StmtKind::If { cond, then, els } => {
            collect_in_expr(cond, out);
            collect_in_stmt(then, out);
            if let Some(e) = els {
                collect_in_stmt(e, out);
            }
        }
        StmtKind::While { cond, body } => {
            collect_in_expr(cond, out);
            collect_in_stmt(body, out);
        }
        StmtKind::ForC {
            init,
            cond,
            step,
            body,
        } => {
            if let Some(i) = init {
                collect_in_stmt(i, out);
            }
            if let Some(c) = cond {
                collect_in_expr(c, out);
            }
            if let Some(st) = step {
                collect_in_expr(st, out);
            }
            collect_in_stmt(body, out);
        }
        StmtKind::ForIn { iter, body, .. } => {
            collect_in_expr(iter, out);
            collect_in_stmt(body, out);
        }
        StmtKind::MatchStmt { subject, arms } => {
            collect_in_expr(subject, out);
            for a in arms {
                match &a.body {
                    MatchBody::Expr(e) => collect_in_expr(e, out),
                    MatchBody::Block(b) => collect_functions(b, out),
                }
            }
        }
        StmtKind::Return(e) => {
            if let Some(x) = e {
                collect_in_expr(x, out);
            }
        }
        StmtKind::Enum { .. } | StmtKind::Import { .. } | StmtKind::Break | StmtKind::Continue => {}
    }
}

fn collect_in_expr(e: &Expr, out: &mut Vec<Rc<FuncDef>>) {
    match &e.node {
        ExprKind::FuncLit(f) => {
            out.push(f.clone());
            collect_functions(&f.body, out);
        }
        ExprKind::Array(xs) => {
            for x in xs {
                collect_in_expr(x, out);
            }
        }
        ExprKind::Object(ps) => {
            for (_, x) in ps {
                collect_in_expr(x, out);
            }
        }
        ExprKind::Unary(_, x) | ExprKind::Borrow { expr: x, .. } => collect_in_expr(x, out),
        ExprKind::Binary(_, a, b) | ExprKind::Logical(_, a, b) => {
            collect_in_expr(a, out);
            collect_in_expr(b, out);
        }
        ExprKind::Ternary(a, b, c) => {
            collect_in_expr(a, out);
            collect_in_expr(b, out);
            collect_in_expr(c, out);
        }
        ExprKind::Assign { target, value, .. } => {
            collect_in_expr(value, out);
            match target {
                AssignTarget::Index(o, i) => {
                    collect_in_expr(o, out);
                    collect_in_expr(i, out);
                }
                AssignTarget::Member(o, _) => collect_in_expr(o, out),
                AssignTarget::Ident(_) => {}
            }
        }
        ExprKind::Call(c, xs) => {
            collect_in_expr(c, out);
            for x in xs {
                collect_in_expr(x, out);
            }
        }
        ExprKind::Index(a, b) => {
            collect_in_expr(a, out);
            collect_in_expr(b, out);
        }
        ExprKind::Slice { obj, start, end } => {
            collect_in_expr(obj, out);
            if let Some(s) = start {
                collect_in_expr(s, out);
            }
            if let Some(e2) = end {
                collect_in_expr(e2, out);
            }
        }
        ExprKind::Member(o, _) => collect_in_expr(o, out),
        ExprKind::Match { subject, arms } => {
            collect_in_expr(subject, out);
            for a in arms {
                if let MatchBody::Expr(ex) = &a.body {
                    collect_in_expr(ex, out);
                }
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// flat locals of a function body
// ---------------------------------------------------------------------------

fn collect_locals(
    stmts: &[Stmt],
    seen: &mut HashSet<String>,
    out: &mut Vec<String>,
    errors: &mut Vec<ResErr>,
) {
    fn add(
        name: &str,
        seen: &mut HashSet<String>,
        out: &mut Vec<String>,
        errors: &mut Vec<ResErr>,
        sp: crate::token::Span,
    ) {
        if !seen.insert(name.to_string()) {
            errors.push(ResErr {
                msg: format!(
                    "duplicate local \"{}\" (native mode: function bodies are a flat namespace; rename one)",
                    name
                ),
                line: sp.line,
                col: sp.col,
            });
        } else {
            out.push(name.to_string());
        }
    }
    for s in stmts {
        if let StmtKind::Func(_) = s.node {
            continue; // nested fns have their own scopes
        }
        match &s.node {
            StmtKind::Var { name, .. } => add(name, seen, out, errors, s.span),
            StmtKind::Import { alias, python, .. } if *python => {
                // `import py "…" as x` is a runtime action — allowed inside
                // blocks/functions (conditional python imports); bind locally
                add(alias, seen, out, errors, s.span)
            }
            StmtKind::Block(b) => collect_locals(b, seen, out, errors),
            StmtKind::If { then, els, .. } => {
                collect_locals(std::slice::from_ref(then), seen, out, errors);
                if let Some(e) = els {
                    collect_locals(std::slice::from_ref(e), seen, out, errors);
                }
            }
            StmtKind::While { body, .. } => {
                collect_locals(std::slice::from_ref(body), seen, out, errors)
            }
            StmtKind::ForC { init, body, .. } => {
                if let Some(i) = init {
                    collect_locals(std::slice::from_ref(i), seen, out, errors);
                }
                collect_locals(std::slice::from_ref(body), seen, out, errors);
            }
            StmtKind::ForIn { name, body, .. } => {
                add(name, seen, out, errors, s.span);
                collect_locals(std::slice::from_ref(body), seen, out, errors);
            }
            StmtKind::MatchStmt { arms, .. } => {
                for a in arms {
                    for p in &a.pats {
                        let mut bs = Vec::new();
                        pattern_binders(p, &mut bs);
                        for n in bs {
                            add(&n, seen, out, errors, a.span);
                        }
                    }
                    if let MatchBody::Block(b) = &a.body {
                        collect_locals(b, seen, out, errors);
                    }
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// capture analysis
// ---------------------------------------------------------------------------

fn pattern_binders(p: &Pattern, out: &mut Vec<String>) {
    match p {
        Pattern::Ident(n) => out.push(n.clone()),
        Pattern::Variant(_, args) => {
            for a in args {
                pattern_binders(a, out);
            }
        }
        _ => {}
    }
}

type Frames = Vec<(Option<usize>, HashSet<String>)>; // (fn_id or global, locals)

struct Cap<'a> {
    fnres: &'a mut HashMap<usize, FnRes>,
}

impl<'a> Cap<'a> {
    fn resolve_frame(frames: &Frames, name: &str) -> Option<usize> {
        for (i, (_, set)) in frames.iter().enumerate().rev() {
            if set.contains(name) {
                return Some(i);
            }
        }
        None
    }

    fn mark_use(&mut self, frames: &Frames, name: &str, user_fn: usize) {
        let Some(fi) = Self::resolve_frame(frames, name) else {
            return;
        };
        let owner = frames[fi].0;
        if owner.is_none() {
            return; // global frame: accessible directly
        }
        let owner = owner.unwrap();
        if owner == user_fn {
            return; // own local
        }
        // owner must keep it in a cell …
        if let Some(r) = self.fnres.get_mut(&owner) {
            r.cell_vars.insert(name.to_string());
        }
        // … and every function between owner and user captures it
        for j in (fi + 1)..frames.len() {
            if let Some(mid) = frames[j].0 {
                if mid != owner {
                    if let Some(r) = self.fnres.get_mut(&mid) {
                        if !r.captures.iter().any(|c| c == name) {
                            r.captures.push(name.to_string());
                        }
                    }
                }
            }
        }
    }

    fn walk_fn(&mut self, def: &Rc<FuncDef>, frames: &mut Frames) {
        let id = FuncDef::id(def);
        let localset: HashSet<String> = self
            .fnres
            .get(&id)
            .map(|r| r.locals.iter().cloned().collect())
            .unwrap_or_default();
        frames.push((Some(id), localset));
        self.walk_stmts(&def.body, frames, id);
        frames.pop();
    }

    fn walk_stmts(&mut self, stmts: &[Stmt], frames: &mut Frames, cur: usize) {
        for s in stmts {
            self.walk_stmt(s, frames, cur);
        }
    }

    fn walk_stmt(&mut self, s: &Stmt, frames: &mut Frames, cur: usize) {
        match &s.node {
            StmtKind::Func(f) => self.walk_fn(f, frames),
            StmtKind::Struct { fields, .. } => {
                for f in fields {
                    if let Some(d) = &f.default {
                        self.walk_expr(d, frames, cur);
                    }
                }
            }
            StmtKind::Impl { methods, .. } | StmtKind::Trait { methods, .. } => {
                for m in methods {
                    self.walk_fn(m, frames);
                }
            }
            StmtKind::Var { value, .. } => self.walk_expr(value, frames, cur),
            StmtKind::ExprStmt(e) => self.walk_expr(e, frames, cur),
            StmtKind::Block(b) => self.walk_stmts(b, frames, cur),
            StmtKind::If { cond, then, els } => {
                self.walk_expr(cond, frames, cur);
                self.walk_stmt(then, frames, cur);
                if let Some(e) = els {
                    self.walk_stmt(e, frames, cur);
                }
            }
            StmtKind::While { cond, body } => {
                self.walk_expr(cond, frames, cur);
                self.walk_stmt(body, frames, cur);
            }
            StmtKind::ForC {
                init,
                cond,
                step,
                body,
            } => {
                if let Some(i) = init {
                    self.walk_stmt(i, frames, cur);
                }
                if let Some(c) = cond {
                    self.walk_expr(c, frames, cur);
                }
                if let Some(st) = step {
                    self.walk_expr(st, frames, cur);
                }
                self.walk_stmt(body, frames, cur);
            }
            StmtKind::ForIn { iter, body, .. } => {
                self.walk_expr(iter, frames, cur);
                self.walk_stmt(body, frames, cur);
            }
            StmtKind::MatchStmt { subject, arms } => {
                self.walk_expr(subject, frames, cur);
                for a in arms {
                    match &a.body {
                        MatchBody::Expr(e) => self.walk_expr(e, frames, cur),
                        MatchBody::Block(b) => self.walk_stmts(b, frames, cur),
                    }
                }
            }
            StmtKind::Return(e) => {
                if let Some(x) = e {
                    self.walk_expr(x, frames, cur);
                }
            }
            StmtKind::Enum { .. }
            | StmtKind::Import { .. }
            | StmtKind::Break
            | StmtKind::Continue => {}
        }
    }

    fn walk_expr(&mut self, e: &Expr, frames: &mut Frames, cur: usize) {
        match &e.node {
            ExprKind::Ident(n) => self.mark_use(frames, n, cur),
            ExprKind::FuncLit(f) => self.walk_fn(f, frames),
            ExprKind::Array(xs) => {
                for x in xs {
                    self.walk_expr(x, frames, cur);
                }
            }
            ExprKind::Object(ps) => {
                for (_, x) in ps {
                    self.walk_expr(x, frames, cur);
                }
            }
            ExprKind::Unary(_, x) | ExprKind::Borrow { expr: x, .. } => {
                self.walk_expr(x, frames, cur)
            }
            ExprKind::Binary(_, a, b) | ExprKind::Logical(_, a, b) => {
                self.walk_expr(a, frames, cur);
                self.walk_expr(b, frames, cur);
            }
            ExprKind::Ternary(a, b, c) => {
                self.walk_expr(a, frames, cur);
                self.walk_expr(b, frames, cur);
                self.walk_expr(c, frames, cur);
            }
            ExprKind::Assign { target, value, .. } => {
                if let AssignTarget::Ident(n) = target {
                    self.mark_use(frames, n, cur);
                }
                match target {
                    AssignTarget::Index(o, i) => {
                        self.walk_expr(o, frames, cur);
                        self.walk_expr(i, frames, cur);
                    }
                    AssignTarget::Member(o, _) => self.walk_expr(o, frames, cur),
                    AssignTarget::Ident(_) => {}
                }
                self.walk_expr(value, frames, cur);
            }
            ExprKind::Call(c, xs) => {
                self.walk_expr(c, frames, cur);
                for x in xs {
                    self.walk_expr(x, frames, cur);
                }
            }
            ExprKind::Index(a, b) => {
                self.walk_expr(a, frames, cur);
                self.walk_expr(b, frames, cur);
            }
            ExprKind::Slice { obj, start, end } => {
                self.walk_expr(obj, frames, cur);
                if let Some(st) = start {
                    self.walk_expr(st, frames, cur);
                }
                if let Some(en) = end {
                    self.walk_expr(en, frames, cur);
                }
            }
            ExprKind::Member(o, _) => self.walk_expr(o, frames, cur),
            ExprKind::Match { subject, arms } => {
                self.walk_expr(subject, frames, cur);
                for a in arms {
                    if let MatchBody::Expr(ex) = &a.body {
                        self.walk_expr(ex, frames, cur);
                    }
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// entry point
// ---------------------------------------------------------------------------

pub fn resolve_program_with_base(
    stmts: &[Stmt],
    builtin_names: &[String],
    global_base: usize,
) -> Result<Resolution, Vec<ResErr>> {
    let mut errors: Vec<ResErr> = Vec::new();

    // struct/impl/trait declarations are top-level only (like Rust items)
    enforce_top_level_items(stmts, &mut errors);

    // all functions
    let mut fdefs: Vec<Rc<FuncDef>> = Vec::new();
    collect_functions(stmts, &mut fdefs);

    // per-fn flat locals
    let mut fnres: HashMap<usize, FnRes> = HashMap::new();
    for f in &fdefs {
        let mut seen: HashSet<String> = HashSet::new();
        let mut locals: Vec<String> = Vec::new();
        for p in &f.params {
            if !seen.insert(p.name.clone()) {
                errors.push(ResErr {
                    msg: format!("duplicate parameter \"{}\"", p.name),
                    line: f.span.line,
                    col: f.span.col,
                });
            } else {
                locals.push(p.name.clone());
            }
        }
        collect_locals(&f.body, &mut seen, &mut locals, &mut errors);
        fnres.insert(
            FuncDef::id(f),
            FnRes {
                captures: Vec::new(),
                cell_vars: HashSet::new(),
                locals,
            },
        );
    }

    // top-level (depth-0) names are globals for this unit
    let mut user_globals: Vec<String> = Vec::new();
    for s in stmts {
        match &s.node {
            StmtKind::Var { name, .. } => user_globals.push(name.clone()),
            StmtKind::Func(f) => user_globals.push(f.name.clone()),
            // a struct binds a runtime type-object under its name; traits are
            // compile-time entities (no slot)
            StmtKind::Struct { name, .. } => user_globals.push(name.clone()),
            StmtKind::Enum { variants, .. } => {
                for v in variants {
                    if v.fields.is_empty() {
                        user_globals.push(v.name.clone());
                    }
                }
            }
            StmtKind::Import {
                module,
                alias,
                python,
            } => {
                // `import "fs";` under its own name needs no slot: the builtin
                // module map already is the global binding
                let native_self_name =
                    !python && !module.ends_with(".px") && alias == module.trim_end_matches(".px");
                if !native_self_name {
                    user_globals.push(alias.clone());
                }
            }
            _ => {}
        }
    }
    let globalframe: HashSet<String> = user_globals.iter().cloned().collect();

    // main pseudo-fn locals: everything declared deeper than depth 0
    let mut main_seen: HashSet<String> = HashSet::new();
    let mut main_locals: Vec<String> = Vec::new();
    collect_main_locals(stmts, &mut main_seen, &mut main_locals, &mut errors);
    if !errors.is_empty() {
        return Err(errors);
    }
    fnres.insert(
        MAIN_RES_ID,
        FnRes {
            captures: Vec::new(),
            cell_vars: HashSet::new(),
            locals: main_locals.clone(),
        },
    );

    // capture analysis
    {
        let mut cap = Cap { fnres: &mut fnres };
        // top-level statements run inside the main pseudo-fn
        let mut frames: Frames = vec![
            (None, globalframe.clone()),
            (Some(MAIN_RES_ID), main_locals.iter().cloned().collect()),
        ];
        cap.walk_stmts(stmts, &mut frames, MAIN_RES_ID);
        // and every function independently under the global frame
        for f in &fdefs {
            // methods (impl/trait) see globals only — they never capture
            // unit locals, like Rust's impl blocks
            let mut frames2: Frames = if f.is_method {
                vec![(None, globalframe.clone())]
            } else {
                vec![
                    (None, globalframe.clone()),
                    (Some(MAIN_RES_ID), main_locals.iter().cloned().collect()),
                ]
            };
            cap.walk_fn(f, &mut frames2);
        }
    }

    let mut globals: HashMap<String, usize> = HashMap::new();
    for (i, n) in builtin_names.iter().enumerate() {
        globals.insert(n.clone(), i);
    }
    for (j, n) in user_globals.iter().enumerate() {
        if globals.contains_key(n) {
            errors.push(ResErr {
                msg: format!(
                    "global \"{}\" collides with a builtin or is declared twice",
                    n
                ),
                line: 0,
                col: 0,
            });
        } else {
            globals.insert(n.clone(), global_base + j);
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    Ok(Resolution {
        fns: fnres,
        globals,
        user_globals,
    })
}

#[allow(dead_code)]
pub fn resolve_program(
    stmts: &[Stmt],
    builtin_names: &[String],
) -> Result<Resolution, Vec<ResErr>> {
    resolve_program_with_base(stmts, builtin_names, builtin_names.len())
}

/// struct/impl/trait are item declarations and must sit at the unit's top
/// level; anywhere deeper is a hard error in both modes
fn enforce_top_level_items(stmts: &[Stmt], errors: &mut Vec<ResErr>) {
    fn rec(stmts: &[Stmt], depth: usize, errors: &mut Vec<ResErr>) {
        for s in stmts {
            match &s.node {
                StmtKind::Struct { name, .. } if depth > 0 => errors.push(ResErr {
                    msg: format!(
                        "struct \"{}\" must be declared at the top level (move it out of the block/function)",
                        name
                    ),
                    line: s.span.line,
                    col: s.span.col,
                }),
                StmtKind::Impl { target, .. } if depth > 0 => errors.push(ResErr {
                    msg: format!(
                        "impl {} must be declared at the top level",
                        target
                    ),
                    line: s.span.line,
                    col: s.span.col,
                }),
                StmtKind::Trait { name, .. } if depth > 0 => errors.push(ResErr {
                    msg: format!(
                        "trait \"{}\" must be declared at the top level",
                        name
                    ),
                    line: s.span.line,
                    col: s.span.col,
                }),
                StmtKind::Enum { name, .. } if depth > 0 => errors.push(ResErr {
                    msg: format!(
                        "enum \"{}\" must be declared at the top level",
                        name
                    ),
                    line: s.span.line,
                    col: s.span.col,
                }),
                StmtKind::Block(b) => rec(b, depth + 1, errors),
                StmtKind::If { then, els, .. } => {
                    rec(std::slice::from_ref(then), depth + 1, errors);
                    if let Some(e) = els {
                        rec(std::slice::from_ref(e), depth + 1, errors);
                    }
                }
                StmtKind::While { body, .. } => rec(std::slice::from_ref(body), depth + 1, errors),
                StmtKind::ForC { init, body, .. } => {
                    if let Some(i) = init {
                        rec(std::slice::from_ref(i), depth + 1, errors);
                    }
                    rec(std::slice::from_ref(body), depth + 1, errors);
                }
                StmtKind::ForIn { body, .. } => rec(std::slice::from_ref(body), depth + 1, errors),
                StmtKind::MatchStmt { arms, .. } => {
                    for a in arms {
                        if let MatchBody::Block(b) = &a.body {
                            rec(b, depth + 1, errors);
                        }
                    }
                }
                StmtKind::Func(f) => rec(&f.body, depth + 1, errors),
                _ => {}
            }
        }
    }
    rec(stmts, 0, errors);
}

/// locals of the unit's pseudo function (block-level names at depth > 0,
/// for-in vars, match bindings, nested func names at depth > 0)
fn collect_main_locals(
    stmts: &[Stmt],
    seen: &mut HashSet<String>,
    out: &mut Vec<String>,
    errors: &mut Vec<ResErr>,
) {
    fn add(
        name: &str,
        seen: &mut HashSet<String>,
        out: &mut Vec<String>,
        errors: &mut Vec<ResErr>,
        sp: crate::token::Span,
    ) {
        if !seen.insert(name.to_string()) {
            errors.push(ResErr {
                msg: format!(
                    "duplicate block-level name \"{}\" (native mode: rename it)",
                    name
                ),
                line: sp.line,
                col: sp.col,
            });
        } else {
            out.push(name.to_string());
        }
    }
    fn rec(
        stmts: &[Stmt],
        depth: usize,
        seen: &mut HashSet<String>,
        out: &mut Vec<String>,
        errors: &mut Vec<ResErr>,
    ) {
        for s in stmts {
            match &s.node {
                StmtKind::Var { name, .. } if depth > 0 => add(name, seen, out, errors, s.span),
                StmtKind::Func(f) => {
                    if depth > 0 {
                        add(&f.name, seen, out, errors, s.span);
                    }
                    // never recurse into fn bodies (own scope)
                    let _ = f;
                }
                StmtKind::Import { alias, python, .. } if *python && depth > 0 => {
                    // conditional python import: bind in the local namespace
                    add(alias, seen, out, errors, s.span)
                }
                StmtKind::Block(b) => rec(b, depth + 1, seen, out, errors),
                StmtKind::If { then, els, .. } => {
                    rec(std::slice::from_ref(then), depth + 1, seen, out, errors);
                    if let Some(e) = els {
                        rec(std::slice::from_ref(e), depth + 1, seen, out, errors);
                    }
                }
                StmtKind::While { body, .. } => {
                    rec(std::slice::from_ref(body), depth + 1, seen, out, errors)
                }
                StmtKind::ForC { init, body, .. } => {
                    if let Some(i) = init {
                        // init var decls are inside the for scope
                        rec(std::slice::from_ref(i), depth + 1, seen, out, errors);
                    }
                    rec(std::slice::from_ref(body), depth + 1, seen, out, errors);
                }
                StmtKind::ForIn { name, body, .. } => {
                    add(name, seen, out, errors, s.span);
                    rec(std::slice::from_ref(body), depth + 1, seen, out, errors);
                }
                StmtKind::MatchStmt { arms, .. } => {
                    for a in arms {
                        for p in &a.pats {
                            let mut bs = Vec::new();
                            pattern_binders(p, &mut bs);
                            for n in bs {
                                add(&n, seen, out, errors, a.span);
                            }
                        }
                        if let MatchBody::Block(b) = &a.body {
                            rec(b, depth + 1, seen, out, errors);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    rec(stmts, 0, seen, out, errors);
}
