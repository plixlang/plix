use crate::ast::*;
use crate::{owncheck, parser, typecheck};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct Warning {
    pub code: &'static str,
    pub msg: String,
    pub line: u32,
    pub col: u32,
}

pub fn lint_source(src: &str, name: &str) -> Result<Vec<Warning>, String> {
    let stmts = parser::parse_file(src).map_err(|e| {
        format!(
            "{}:{}:{}: syntax error: {}",
            name, e.span.line, e.span.col, e.msg
        )
    })?;
    typecheck::check_program(&stmts).map_err(|errs| owncheck::format_errors(&errs, src, name))?;
    owncheck::check_program(&stmts).map_err(|errs| owncheck::format_errors(&errs, src, name))?;
    let mut l = Linter::default();
    l.scan_stmts(&stmts);
    Ok(l.finish())
}

#[derive(Default)]
struct Linter {
    decls: HashMap<String, (u32, u32, &'static str)>,
    uses: HashSet<String>,
    warnings: Vec<Warning>,
}

impl Linter {
    fn declare(&mut self, name: &str, s: crate::token::Span, kind: &'static str) {
        if !name.starts_with('_') && !(kind == "function" && name.starts_with("test_")) {
            self.decls
                .entry(name.to_string())
                .or_insert((s.line, s.col, kind));
        }
    }
    fn use_name(&mut self, name: &str) {
        self.uses.insert(name.to_string());
    }
    fn warn(&mut self, code: &'static str, msg: impl Into<String>, s: crate::token::Span) {
        self.warnings.push(Warning {
            code,
            msg: msg.into(),
            line: s.line,
            col: s.col,
        });
    }
    fn finish(mut self) -> Vec<Warning> {
        for (name, (line, col, kind)) in self.decls {
            if !self.uses.contains(&name) {
                self.warnings.push(Warning {
                    code: match kind {
                        "import" => "W0002",
                        _ => "W0001",
                    },
                    msg: format!("unused {} \"{}\"", kind, name),
                    line,
                    col,
                });
            }
        }
        self.warnings.sort_by_key(|w| (w.line, w.col, w.code));
        self.warnings
    }
    fn scan_stmts(&mut self, stmts: &[Stmt]) {
        let mut unreachable = false;
        for s in stmts {
            if unreachable {
                self.warn("W0003", "unreachable statement", s.span);
            }
            self.scan_stmt(s);
            if matches!(
                s.node,
                StmtKind::Return(_) | StmtKind::Break | StmtKind::Continue
            ) {
                unreachable = true;
            }
        }
    }
    fn scan_stmt(&mut self, s: &Stmt) {
        match &s.node {
            StmtKind::Var { name, value, .. } => {
                self.scan_expr(value);
                self.declare(name, s.span, "variable");
            }
            StmtKind::Func(f) => {
                self.declare(&f.name, s.span, "function");
                for p in &f.params {
                    self.declare(&p.name, f.span, "parameter");
                    if let Some(d) = &p.default {
                        self.scan_expr(d);
                    }
                }
                self.scan_stmts(&f.body);
            }
            StmtKind::Import { alias, .. } => self.declare(alias, s.span, "import"),
            StmtKind::Struct { fields, .. } => {
                for f in fields {
                    if let Some(d) = &f.default {
                        self.scan_expr(d);
                    }
                }
            }
            StmtKind::Enum { .. } | StmtKind::Trait { .. } => {}
            StmtKind::Impl { methods, .. } => {
                for m in methods {
                    self.scan_stmts(&m.body);
                }
            }
            StmtKind::ExprStmt(e) => self.scan_expr(e),
            StmtKind::Block(b) => self.scan_stmts(b),
            StmtKind::If { cond, then, els } => {
                self.scan_expr(cond);
                self.scan_stmt(then);
                if let Some(e) = els {
                    self.scan_stmt(e);
                }
            }
            StmtKind::While { cond, body } => {
                self.scan_expr(cond);
                self.scan_stmt(body);
            }
            StmtKind::ForC {
                init,
                cond,
                step,
                body,
            } => {
                if let Some(i) = init {
                    self.scan_stmt(i);
                }
                if let Some(c) = cond {
                    self.scan_expr(c);
                }
                if let Some(st) = step {
                    self.scan_expr(st);
                }
                self.scan_stmt(body);
            }
            StmtKind::ForIn {
                name, iter, body, ..
            } => {
                self.scan_expr(iter);
                self.declare(name, s.span, "variable");
                self.scan_stmt(body);
            }
            StmtKind::MatchStmt { subject, arms } => {
                self.scan_expr(subject);
                for a in arms {
                    for p in &a.pats {
                        self.scan_pattern(p);
                    }
                    match &a.body {
                        MatchBody::Expr(e) => self.scan_expr(e),
                        MatchBody::Block(b) => self.scan_stmts(b),
                    }
                }
            }
            StmtKind::Return(e) => {
                if let Some(e) = e {
                    self.scan_expr(e);
                }
            }
            StmtKind::Break | StmtKind::Continue => {}
        }
    }
    fn scan_pattern(&mut self, p: &Pattern) {
        match p {
            Pattern::Ident(n) => {
                self.decls.entry(n.clone()).or_insert((0, 0, "binding"));
            }
            Pattern::Variant(_, args) => {
                for a in args {
                    self.scan_pattern(a);
                }
            }
            _ => {}
        }
    }
    fn scan_expr(&mut self, e: &Expr) {
        match &e.node {
            ExprKind::Ident(n) => self.use_name(n),
            ExprKind::Array(xs) => xs.iter().for_each(|x| self.scan_expr(x)),
            ExprKind::Object(ps) => ps.iter().for_each(|(_, x)| self.scan_expr(x)),
            ExprKind::Unary(_, x) | ExprKind::Borrow { expr: x, .. } => self.scan_expr(x),
            ExprKind::Binary(_, a, b) | ExprKind::Logical(_, a, b) => {
                self.scan_expr(a);
                self.scan_expr(b);
            }
            ExprKind::Ternary(a, b, c) => {
                self.scan_expr(a);
                self.scan_expr(b);
                self.scan_expr(c);
            }
            ExprKind::Assign { target, value, .. } => {
                self.scan_target(target);
                self.scan_expr(value);
            }
            ExprKind::Call(c, args) => {
                self.scan_expr(c);
                args.iter().for_each(|a| self.scan_expr(a));
            }
            ExprKind::Index(a, b) => {
                self.scan_expr(a);
                self.scan_expr(b);
            }
            ExprKind::Slice { obj, start, end } => {
                self.scan_expr(obj);
                if let Some(s) = start {
                    self.scan_expr(s);
                }
                if let Some(e) = end {
                    self.scan_expr(e);
                }
            }
            ExprKind::Member(o, _) => self.scan_expr(o),
            ExprKind::FuncLit(f) => self.scan_stmts(&f.body),
            ExprKind::Match { subject, arms } => {
                self.scan_expr(subject);
                for a in arms {
                    for p in &a.pats {
                        self.scan_pattern(p);
                    }
                    match &a.body {
                        MatchBody::Expr(e) => self.scan_expr(e),
                        MatchBody::Block(b) => self.scan_stmts(b),
                    }
                }
            }
            ExprKind::StructLit { fields, .. } => {
                fields.iter().for_each(|(_, e)| self.scan_expr(e));
            }
            ExprKind::Null
            | ExprKind::Bool(_)
            | ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Str(_) => {}
        }
    }
    fn scan_target(&mut self, t: &AssignTarget) {
        match t {
            AssignTarget::Ident(n) => self.use_name(n),
            AssignTarget::Index(a, b) => {
                self.scan_expr(a);
                self.scan_expr(b);
            }
            AssignTarget::Member(o, _) => self.scan_expr(o),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lint_unused_variable() {
        let src = "auto x = 1;";
        let warnings = lint_source(src, "test.px").unwrap();
        assert!(warnings.iter().any(|w| w.code == "W0001" && w.msg.contains("unused")),
            "expected unused variable warning, got: {warnings:?}");
    }

    #[test]
    fn lint_used_variable_no_warning() {
        let src = "auto x = 1; say(x);";
        let warnings = lint_source(src, "test.px").unwrap();
        let unused: Vec<_> = warnings.iter().filter(|w| w.code == "W0001").collect();
        assert!(unused.is_empty(), "expected no unused-variable warnings, got: {unused:?}");
    }

    #[test]
    fn lint_unreachable_code() {
        let src = "func f() { return 1; say(2); }";
        let warnings = lint_source(src, "test.px").unwrap();
        assert!(warnings.iter().any(|w| w.code == "W0003"),
            "expected unreachable warning, got: {warnings:?}");
    }

    #[test]
    fn lint_valid_program_no_errors() {
        let src = "auto x = 1; say(x);";
        assert!(lint_source(src, "test.px").is_ok());
    }
}
