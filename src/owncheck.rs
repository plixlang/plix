//! Plix ownership checker — static analysis for `own` variables, in the
//! spirit of Rust's borrow checker, with zero runtime cost.
//!
//! Rules (see docs/memory.md):
//!   - `own x = ...` creates an owning binding. Scalars (int/float/bool/null)
//!     are Copy; arrays, objects, strings, functions move.
//!   - ownership transfer is always *explicit*: `own y = x` moves x,
//!     `return x` moves it, `for (x in xs)` consumes the container (use
//!     `for (x in &xs)` to iterate by borrow). Use-after-move is E0382.
//!   - function *arguments borrow, never move*: a callee reads its
//!     parameters (they behave like `auto` bindings), so calling — even
//!     `say(x)` — leaves your `own` bindings usable.
//!   - `&x` borrows immutably (many allowed), `&mut x` mutably (exclusive);
//!     borrows live until the end of the enclosing statement.
//!   - moving while borrowed = E0503, borrowing a moved value = E0382,
//!     writing while borrowed = E0506, moving in a loop = E0382-loop,
//!     capturing an owned value in a closure = E0373.
//!   - `auto`/`const` values are reference-counted (no rules); the checker
//!     also catches `const` rebinding (E0594).

use crate::ast::*;
use crate::token::Span;

#[derive(Debug, Clone)]
pub struct OwnError {
    pub code: &'static str,
    pub msg: String,
    pub span: Span,
    pub notes: Vec<(String, Span)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum St {
    Live,
    Moved,
    IBorrow(u32),
    MBorrow,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum K {
    Auto,
    Const,
    Own,
}

#[derive(Debug, Clone)]
struct Var {
    k: K,
    st: St,
    copy: bool,
    decl: Span,
    moved_at: Option<Span>,
    reassigned_in_loop: bool,
}

struct Ctx {
    /// scope stack; `fn_boundary` marks where the innermost function starts
    scopes: Vec<std::collections::HashMap<String, Var>>,
    fn_bounds: Vec<usize>,
    errors: Vec<OwnError>,
}

pub fn check_program(stmts: &[Stmt]) -> Result<(), Vec<OwnError>> {
    let mut ctx = Ctx {
        scopes: vec![std::collections::HashMap::new()],
        fn_bounds: vec![0],
        errors: Vec::new(),
    };
    for s in stmts {
        ctx.stmt(s);
    }
    if ctx.errors.is_empty() {
        Ok(())
    } else {
        Err(ctx.errors)
    }
}

pub fn format_errors(errs: &[OwnError], src: &str, file: &str) -> String {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = String::new();
    for e in errs {
        out.push_str(&format!(
            "error[{}]: {}\n  --> {}:{}:{}\n",
            e.code, e.msg, file, e.span.line, e.span.col
        ));
        if let Some(line) = lines.get((e.span.line - 1) as usize) {
            out.push_str(&format!(
                "   |\n{:3}| {}\n   | {}^\n",
                e.span.line,
                line,
                " ".repeat((e.span.col - 1) as usize)
            ));
        }
        for (n, sp) in &e.notes {
            out.push_str(&format!("   = note: {} (at {}:{})\n", n, sp.line, sp.col));
        }
    }
    out
}

fn snapshot(scopes: &[std::collections::HashMap<String, Var>]) -> Vec<(String, St)> {
    let mut out = Vec::new();
    for m in scopes {
        for (k, v) in m {
            out.push((k.clone(), v.st));
        }
    }
    out
}

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

impl Ctx {
    // ---------------- variable state ----------------
    fn declare(&mut self, name: &str, k: K, copy: bool, sp: Span) {
        self.scopes.last_mut().unwrap().insert(
            name.to_string(),
            Var {
                k,
                st: St::Live,
                copy,
                decl: sp,
                moved_at: None,
                reassigned_in_loop: false,
            },
        );
    }

    fn lookup_idx(&self, name: &str) -> Option<(usize, &Var)> {
        for (i, m) in self.scopes.iter().enumerate().rev() {
            if let Some(v) = m.get(name) {
                return Some((i, v));
            }
        }
        None
    }

    /// Reading a variable (not moving it). `callee` marks call position /
    /// indexing / member access bases (treated like borrows in Rust's method
    /// calls and are fine for owned values).
    fn read_var(&mut self, name: &str, sp: Span) {
        let Some((i, v)) = self.lookup_idx(name) else {
            return; // undefined names are a runtime/resolve concern
        };
        let (k, st, moved_at, copy, in_outer_fn) = (
            v.k,
            v.st,
            v.moved_at,
            v.copy,
            i < *self.fn_bounds.last().unwrap_or(&0),
        );
        let decl = v.decl;
        if k == K::Own && in_outer_fn {
            self.errors.push(OwnError {
                code: "E0373",
                msg: format!("owned value \"{}\" cannot be captured by a closure", name),
                span: sp,
                notes: vec![
                    (format!("\"{}\" declared as own here", name), decl),
                    ("copy it into an `auto` variable first".into(), sp),
                ],
            });
            return;
        }
        if k != K::Own {
            return;
        }
        match st {
            St::Moved => {
                let mut notes = vec![];
                if let Some(ms) = moved_at {
                    notes.push((format!("\"{}\" was moved here", name), ms));
                }
                self.errors.push(OwnError {
                    code: "E0382",
                    msg: format!("use of moved value \"{}\"", name),
                    span: sp,
                    notes,
                });
            }
            St::MBorrow if !copy => {
                self.errors.push(OwnError {
                    code: "E0502",
                    msg: format!("cannot use \"{}\" while it is mutably borrowed", name),
                    span: sp,
                    notes: vec![],
                });
            }
            _ => {}
        }
    }

    fn get_mut<'a>(&'a mut self, name: &str) -> Option<&'a mut Var> {
        for m in self.scopes.iter_mut().rev() {
            if m.contains_key(name) {
                return m.get_mut(name);
            }
        }
        None
    }

    /// Moving a variable (pass-by-value / own-to-own assign / return).
    fn move_var(&mut self, name: &str, sp: Span) {
        let Some(v) = self.get_mut(name) else { return };
        if v.k != K::Own || v.copy {
            self.read_var_nonmoving(name, sp);
            return;
        }
        match v.st {
            St::Live => {
                v.st = St::Moved;
                v.moved_at = Some(sp);
            }
            St::Moved => {
                let moved_at = v.moved_at;
                let mut notes = vec![];
                if let Some(ms) = moved_at {
                    notes.push((format!("\"{}\" was moved earlier here", name), ms));
                }
                self.errors.push(OwnError {
                    code: "E0382",
                    msg: format!("use of moved value \"{}\"", name),
                    span: sp,
                    notes,
                });
            }
            St::IBorrow(_) => {
                self.errors.push(OwnError {
                    code: "E0503",
                    msg: format!("cannot move \"{}\" because it is borrowed", name),
                    span: sp,
                    notes: vec![("borrow ends at the end of this statement".into(), sp)],
                });
            }
            St::MBorrow => {
                self.errors.push(OwnError {
                    code: "E0503",
                    msg: format!("cannot move \"{}\" because it is mutably borrowed", name),
                    span: sp,
                    notes: vec![],
                });
            }
        }
    }

    fn read_var_nonmoving(&mut self, name: &str, sp: Span) {
        let Some((_, v)) = self.lookup_idx(name) else {
            return;
        };
        if v.k == K::Own && v.st == St::Moved {
            let moved_at = v.moved_at;
            let mut notes = vec![];
            if let Some(ms) = moved_at {
                notes.push((format!("\"{}\" was moved here", name), ms));
            }
            self.errors.push(OwnError {
                code: "E0382",
                msg: format!("use of moved value \"{}\"", name),
                span: sp,
                notes,
            });
        }
    }

    fn borrow(&mut self, name: &str, mutable: bool, sp: Span) {
        let Some(v) = self.get_mut(name) else { return };
        if v.k != K::Own {
            return;
        }
        match (v.st, mutable) {
            (St::Moved, _) => {
                let moved_at = v.moved_at;
                self.errors.push(OwnError {
                    code: "E0382",
                    msg: format!("cannot borrow \"{}\": value was moved", name),
                    span: sp,
                    notes: moved_at
                        .map(|ms| vec![("moved here".to_string(), ms)])
                        .unwrap_or_default(),
                });
            }
            (St::Live, false) => v.st = St::IBorrow(1),
            (St::Live, true) => v.st = St::MBorrow,
            (St::IBorrow(n), false) => v.st = St::IBorrow(n + 1),
            (St::IBorrow(_), true) => {
                self.errors.push(OwnError {
                    code: "E0502",
                    msg: format!(
                        "cannot mutably borrow \"{}\" while immutable borrows are active",
                        name
                    ),
                    span: sp,
                    notes: vec![],
                });
            }
            (St::MBorrow, _) => {
                self.errors.push(OwnError {
                    code: "E0499",
                    msg: format!("cannot borrow \"{}\" more than once mutably", name),
                    span: sp,
                    notes: vec![],
                });
            }
        }
    }

    /// end of statement: all borrows expire
    fn reset_borrows(&mut self) {
        for m in self.scopes.iter_mut() {
            for (_, v) in m.iter_mut() {
                match v.st {
                    St::IBorrow(_) | St::MBorrow => v.st = St::Live,
                    _ => {}
                }
            }
        }
    }

    fn assign_var(&mut self, name: &str, sp: Span) {
        enum Act {
            None,
            ConstRebind(Span),
            BorrowedWrite,
        }
        let mut act = Act::None;
        {
            let Some(v) = self.get_mut(name) else { return };
            match v.k {
                K::Const => {
                    act = Act::ConstRebind(v.decl);
                }
                K::Own => {
                    if matches!(v.st, St::IBorrow(_) | St::MBorrow) {
                        act = Act::BorrowedWrite;
                    }
                }
                K::Auto => {}
            }
            // assignment (re)initializes the variable
            if v.k == K::Own {
                v.st = St::Live;
                v.moved_at = None;
                v.reassigned_in_loop = true;
            }
        }
        match act {
            Act::None => {}
            Act::ConstRebind(decl) => self.errors.push(OwnError {
                code: "E0594",
                msg: format!("cannot assign to const \"{}\"", name),
                span: sp,
                notes: vec![("declared const here".into(), decl)],
            }),
            Act::BorrowedWrite => self.errors.push(OwnError {
                code: "E0506",
                msg: format!("cannot assign to \"{}\" while it is borrowed", name),
                span: sp,
                notes: vec![],
            }),
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(std::collections::HashMap::new());
    }
    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    // ---------------- type triviality ----------------
    fn is_copy_expr(&self, e: &Expr) -> bool {
        match &e.node {
            ExprKind::Null | ExprKind::Bool(_) | ExprKind::Int(_) | ExprKind::Float(_) => true,
            ExprKind::Unary(_, x) => self.is_copy_expr(x),
            ExprKind::Binary(_, a, b) => self.is_copy_expr(a) && self.is_copy_expr(b),
            ExprKind::Ternary(_, a, b) => self.is_copy_expr(a) && self.is_copy_expr(b),
            ExprKind::Ident(n) => self.lookup_idx(n).map(|(_, v)| v.copy).unwrap_or(true),
            _ => false,
        }
    }

    // ---------------- statements ----------------
    /// Returns true when the statement *diverges* — i.e. definitely does not
    /// fall through normally (returns on every path). Used so that
    /// `if (c) { return x; } use_after(x);` stays legal.
    fn stmt(&mut self, s: &Stmt) -> bool {
        let diverges = self.stmt_inner(s);
        self.reset_borrows();
        diverges
    }

    fn stmt_inner(&mut self, s: &Stmt) -> bool {
        match &s.node {
            StmtKind::Var {
                kind, name, value, ..
            } => {
                let copy = self.is_copy_expr(value);
                self.expr_moves_context(value, matches!(kind, VarKind::Own));
                self.declare(
                    name,
                    match kind {
                        VarKind::Auto => K::Auto,
                        VarKind::Const => K::Const,
                        VarKind::Own => K::Own,
                    },
                    copy,
                    s.span,
                );
            }
            StmtKind::Func(def) => {
                self.declare(&def.name, K::Const, true, s.span);
                self.check_fn(def);
            }
            StmtKind::Struct { fields, .. } => {
                for f in fields {
                    if let Some(d) = &f.default {
                        self.expr(d);
                    }
                }
            }
            StmtKind::Impl { methods, .. } | StmtKind::Trait { methods, .. } => {
                for m in methods {
                    self.check_fn(m);
                }
            }
            StmtKind::Enum { .. } => {}
            StmtKind::Import { .. } => {}
            StmtKind::ExprStmt(e) => self.expr(e),
            StmtKind::Block(stmts) => {
                self.push_scope();
                let mut d = false;
                for st in stmts {
                    if self.stmt(st) {
                        d = true;
                    }
                }
                self.pop_scope();
                self.reset_borrows();
                return d;
            }
            StmtKind::If { cond, then, els } => {
                self.expr(cond);
                let base = snapshot(&self.scopes);
                let d_then = self.stmt(then);
                let after_then = snapshot(&self.scopes);
                let (after_else, d_else) = if let Some(e) = els {
                    self.restore(&base);
                    let d = self.stmt(e);
                    (snapshot(&self.scopes), d)
                } else {
                    (base.clone(), false)
                };
                // a branch that returns/breaks doesn't affect the fallthrough
                let post = match (d_then, d_else) {
                    (true, false) => after_else.clone(),
                    (false, true) => after_then.clone(),
                    (true, true) => base.clone(),
                    (false, false) => merge(&base, &after_then, &after_else),
                };
                self.restore(&post);
                self.reset_borrows();
                return d_then && d_else;
            }
            StmtKind::While { cond, body } => {
                self.expr(cond);
                self.loop_body(body);
            }
            StmtKind::ForC {
                init,
                cond,
                step,
                body,
            } => {
                self.push_scope();
                if let Some(i) = init {
                    self.stmt(i);
                }
                if let Some(c) = cond {
                    self.expr(c);
                }
                if let Some(st) = step {
                    self.expr(st);
                }
                self.loop_body(body);
                self.pop_scope();
            }
            StmtKind::ForIn {
                name, iter, body, ..
            } => {
                // `for (x in own_arr)` moves; `for (x in &own_arr)` borrows
                match &iter.node {
                    ExprKind::Borrow { mutable, expr } => {
                        self.borrow_target(*mutable, expr);
                    }
                    ExprKind::Ident(n) => {
                        if self
                            .lookup_idx(n)
                            .map(|(_, v)| v.k == K::Own && !v.copy)
                            .unwrap_or(false)
                        {
                            self.move_var(n, iter.span);
                        } else {
                            self.expr(iter);
                        }
                    }
                    _ => self.expr(iter),
                }
                self.push_scope();
                self.declare(name, K::Auto, false, s.span);
                self.loop_body(body);
                self.pop_scope();
            }
            StmtKind::MatchStmt { subject, arms } => {
                self.expr(subject);
                for arm in arms {
                    self.push_scope();
                    for p in &arm.pats {
                        let mut bs = Vec::new();
                        pattern_binders(p, &mut bs);
                        for n in bs {
                            self.declare(&n, K::Auto, false, arm.span);
                        }
                    }
                    match &arm.body {
                        MatchBody::Expr(e) => self.expr(e),
                        MatchBody::Block(stmts) => {
                            for st in stmts {
                                self.stmt(st);
                            }
                        }
                    }
                    self.pop_scope();
                }
                // arms are alternatives: moves inside an arm affect control
                // only conservatively — accept both outcomes (like if/else
                // with many branches: handled per-arm by the state reset)
            }
            StmtKind::Return(v) => {
                if let Some(e) = v {
                    // returning an owned binding moves it — always allowed
                    if let ExprKind::Ident(n) = &e.node {
                        if self
                            .lookup_idx(n)
                            .map(|(_, v)| v.k == K::Own && !v.copy)
                            .unwrap_or(false)
                        {
                            self.move_var(n, e.span);
                            self.reset_borrows();
                            return true;
                        }
                    }
                    self.expr(e);
                }
                self.reset_borrows();
                return true;
            }
            StmtKind::Break | StmtKind::Continue => {
                return false;
            }
        }
        false
    }

    fn loop_body(&mut self, body: &Stmt) {
        let header = snapshot(&self.scopes);
        let header_state: std::collections::HashMap<&String, St> =
            header.iter().map(|(n, s)| (n, *s)).collect();
        self.clear_reassign_flags();
        self.stmt(body);
        let end = snapshot(&self.scopes);
        // moving in an iteration without reinitializing is an error — but
        // only for variables that already existed at the loop header and
        // were NOT already moved before entering the loop
        for (name, st_end) in &end {
            if *st_end != St::Moved {
                continue;
            }
            match header_state.get(name) {
                Some(St::Moved) => continue, // moved before the loop: fine
                None => continue,            // declared inside the loop
                _ => {}
            }
            let flagged = match self.get_mut(name) {
                Some(v) if v.k == K::Own && !v.reassigned_in_loop => Some(v.moved_at),
                _ => None,
            };
            if let Some(moved_at) = flagged {
                self.errors.push(OwnError {
                    code: "E0382",
                    msg: format!(
                        "\"{}\" is moved in a loop iteration but not reinitialized",
                        name
                    ),
                    span: moved_at.unwrap_or(body.span),
                    notes: vec![],
                });
            }
        }
        // after the loop, take body-end state (conservative)
        self.restore(&merge(&header, &end, &end));
    }

    fn clear_reassign_flags(&mut self) {
        for m in self.scopes.iter_mut() {
            for (_, v) in m.iter_mut() {
                v.reassigned_in_loop = false;
            }
        }
    }

    fn restore(&mut self, snap: &[(String, St)]) {
        for (name, st) in snap {
            if let Some(v) = self.get_mut(name) {
                v.st = *st;
            }
        }
    }

    fn check_fn(&mut self, def: &FuncDef) {
        let boundary = self.scopes.len();
        self.fn_bounds.push(boundary);
        self.push_scope();
        for p in &def.params {
            // Plix call semantics: arguments are borrowed (ARC-shared), never
            // moved into the callee — so parameters are ordinary automatic
            // bindings (capturable by closures) even when the caller passed
            // an `own` value. Ownership transfer in Plix is always explicit
            // (`own y = x`, or returning an owned local).
            self.declare(&p.name, K::Auto, false, def.span);
            if let Some(d) = &p.default {
                self.expr(d);
            }
        }
        for s in &def.body {
            self.stmt(s);
        }
        self.pop_scope();
        self.fn_bounds.pop();
    }

    // ---------------- expressions ----------------
    /// like `expr`, but owned identifiers in value position MOVE
    /// (used for `own y = x`-style transfers when moves_enabled). Container
    /// literals move their parts, so `own ys = [&xs, xs]` reports E0503.
    /// Call arguments still borrow (calls never consume).
    fn expr_moves_context(&mut self, e: &Expr, moves_enabled: bool) {
        if !moves_enabled {
            self.expr(e);
            return;
        }
        match &e.node {
            ExprKind::Ident(n) => {
                if self
                    .lookup_idx(n)
                    .map(|(_, v)| v.k == K::Own && !v.copy)
                    .unwrap_or(false)
                {
                    self.move_var(n, e.span);
                } else {
                    self.expr(e);
                }
            }
            ExprKind::Borrow { mutable, expr } => self.borrow_target(*mutable, expr),
            ExprKind::Array(xs) => {
                for x in xs {
                    self.expr_moves_context(x, true);
                }
            }
            ExprKind::Object(ps) => {
                for (_, x) in ps {
                    self.expr_moves_context(x, true);
                }
            }
            ExprKind::StructLit { fields, .. } => {
                for (_, x) in fields {
                    self.expr_moves_context(x, true);
                }
            }
            ExprKind::Unary(_, x) => self.expr_moves_context(x, true),
            ExprKind::Binary(_, a, b) | ExprKind::Logical(_, a, b) => {
                self.expr_moves_context(a, true);
                self.expr_moves_context(b, true);
            }
            ExprKind::Ternary(a, b, c) => {
                self.expr_moves_context(a, true);
                self.expr_moves_context(b, true);
                self.expr_moves_context(c, true);
            }
            _ => self.expr(e),
        }
    }

    fn borrow_target(&mut self, mutable: bool, e: &Expr) {
        // resolve to the root identifier of the borrow target
        let mut cur = e;
        loop {
            match &cur.node {
                ExprKind::Ident(n) => {
                    self.borrow(n, mutable, e.span);
                    return;
                }
                ExprKind::Index(o, i) => {
                    self.expr(i);
                    cur = o;
                }
                ExprKind::Member(o, _) => {
                    cur = o;
                }
                ExprKind::Slice { obj, .. } => {
                    cur = obj;
                }
                _ => {
                    self.expr(cur);
                    return;
                }
            }
        }
    }

    fn expr(&mut self, e: &Expr) {
        match &e.node {
            ExprKind::Null
            | ExprKind::Bool(_)
            | ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Str(_) => {}
            ExprKind::Ident(n) => self.read_var(n, e.span),
            ExprKind::Array(items) => {
                for i in items {
                    self.expr(i);
                }
            }
            ExprKind::Object(props) => {
                for (_, v) in props {
                    self.expr(v);
                }
            }
            ExprKind::StructLit { fields, .. } => {
                for (_, v) in fields {
                    self.expr(v);
                }
            }
            ExprKind::Unary(_, x) => self.expr(x),
            ExprKind::Borrow { mutable, expr } => self.borrow_target(*mutable, expr),
            ExprKind::Binary(_, a, b) => {
                self.expr(a);
                self.expr(b);
            }
            ExprKind::Logical(_, a, b) => {
                self.expr(a);
                self.expr(b);
            }
            ExprKind::Ternary(c, a, b) => {
                self.expr(c);
                self.expr(a);
                self.expr(b);
            }
            ExprKind::Assign { target, op, value } => {
                self.expr(value);
                match target {
                    AssignTarget::Ident(n) => self.assign_var(n, e.span),
                    AssignTarget::Index(o, i) => {
                        let _ = op;
                        self.expr(o);
                        self.expr(i);
                    }
                    AssignTarget::Member(o, _) => {
                        self.expr(o);
                    }
                }
            }
            ExprKind::Call(callee, args) => {
                self.expr(callee);
                // arguments borrow: a callee may read its arguments but can
                // never consume the caller's `own` bindings (Rust-style, the
                // type is "immutable reference" unless the callee re-binds
                // with `own` internally)
                for a in args {
                    match &a.node {
                        ExprKind::Borrow { mutable, expr } => {
                            self.borrow_target(*mutable, expr);
                        }
                        ExprKind::Ident(n) => {
                            self.read_var_nonmoving(n, a.span);
                        }
                        _ => self.expr(a),
                    }
                }
            }
            ExprKind::Index(o, i) => {
                self.expr(o);
                self.expr(i);
            }
            ExprKind::Slice { obj, start, end } => {
                self.expr(obj);
                if let Some(s) = start {
                    self.expr(s);
                }
                if let Some(en) = end {
                    self.expr(en);
                }
            }
            ExprKind::Member(o, _) => self.expr(o),
            ExprKind::FuncLit(def) => self.check_fn(def),
            ExprKind::Match { subject, arms } => {
                self.expr(subject);
                for arm in arms {
                    self.push_scope();
                    for p in &arm.pats {
                        let mut bs = Vec::new();
                        pattern_binders(p, &mut bs);
                        for n in bs {
                            self.declare(&n, K::Auto, false, arm.span);
                        }
                    }
                    match &arm.body {
                        MatchBody::Expr(e) => self.expr(e),
                        MatchBody::Block(stmts) => {
                            for st in stmts {
                                self.stmt(st);
                            }
                        }
                    }
                    self.pop_scope();
                }
            }
        }
    }

    #[allow(dead_code)]
    fn span_of(&self, s: &Stmt) -> Span {
        s.span
    }
}

/// three-way branch merge: base (before), a (then), b (else)
fn merge(base: &[(String, St)], a: &[(String, St)], b: &[(String, St)]) -> Vec<(String, St)> {
    let mut out = Vec::new();
    for (name, base_st) in base {
        let sa = a.iter().find(|(n, _)| n == name).map(|(_, s)| *s);
        let sb = b.iter().find(|(n, _)| n == name).map(|(_, s)| *s);
        let merged = match (sa, sb) {
            (Some(x), Some(y)) if x == y => x,
            (Some(St::Moved), Some(_)) | (Some(_), Some(St::Moved)) => St::Moved,
            (Some(x), None) => {
                if x == St::Moved {
                    St::Moved
                } else {
                    *base_st
                }
            }
            (None, Some(y)) => {
                if y == St::Moved {
                    St::Moved
                } else {
                    *base_st
                }
            }
            (Some(x), Some(_)) => x,
            _ => *base_st,
        };
        out.push((name.clone(), merged));
    }
    out
}
