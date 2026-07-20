//! Plix parser: recursive descent with precedence climbing.
//!
//! Covers the complete v0.2 grammar: variables (auto/const/own), functions
//! (default & rest params, anonymous function literals, closures), control
//! flow (if/else-if/else, while, C-style for, for-in, match, break/continue,
//! return), full operator set (ternary, ||, &&, bitwise, compound
//! assignment), arrays/objects, indexing, slicing, member access, string
//! interpolation, imports (plix files, native modules, python).

use crate::ast::*;
use crate::lexer;
use crate::token::{Span, StrPart, Tok, Token};
use std::rc::Rc;

#[derive(Debug)]
pub struct ParseError {
    pub msg: String,
    pub span: Span,
}

type PResult<T> = Result<T, ParseError>;

pub struct Parser {
    toks: Vec<Token>,
    pos: usize,
    /// disables `Ident { .. }` struct-literal parsing (set while parsing a
    /// paren-less match subject so `match p { ... }` is not mis-read)
    no_struct_lit: bool,
}

pub fn parse_file(src: &str) -> Result<Vec<Stmt>, ParseError> {
    let toks = lexer::lex(src).map_err(|e| ParseError {
        msg: e.msg,
        span: Span {
            line: e.line,
            col: e.col,
        },
    })?;
    let mut p = Parser {
        toks,
        pos: 0,
        no_struct_lit: false,
    };
    let mut out = Vec::new();
    while !p.at(&Tok::Eof) {
        out.push(p.declaration()?);
    }
    Ok(out)
}

/// Parse a single expression (used for string interpolation payloads).
pub fn parse_expr_source(src: &str) -> Result<Expr, ParseError> {
    let toks = lexer::lex(src).map_err(|e| ParseError {
        msg: e.msg,
        span: Span {
            line: e.line,
            col: e.col,
        },
    })?;
    let mut p = Parser {
        toks,
        pos: 0,
        no_struct_lit: false,
    };
    let e = p.expression()?;
    if !p.at(&Tok::Eof) {
        return Err(p.error("unexpected tokens after expression"));
    }
    Ok(e)
}

impl Parser {
    // ---------------- utilities ----------------
    fn cur(&self) -> &Token {
        &self.toks[self.pos.min(self.toks.len() - 1)]
    }
    fn span(&self) -> Span {
        self.cur().span
    }
    fn at(&self, t: &Tok) -> bool {
        &self.cur().tok == t
    }
    fn bump(&mut self) -> &Token {
        let t = &self.toks[self.pos.min(self.toks.len() - 1)];
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }
    fn eat(&mut self, t: &Tok) -> bool {
        if self.at(t) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn error(&self, msg: impl Into<String>) -> ParseError {
        ParseError {
            msg: msg.into(),
            span: self.span(),
        }
    }
    fn expect(&mut self, t: &Tok, what: &str) -> PResult<Span> {
        if self.at(t) {
            let s = self.span();
            self.bump();
            Ok(s)
        } else {
            Err(self.error(format!(
                "expected {} (found {})",
                what,
                self.cur().tok.describe()
            )))
        }
    }
    fn expect_ident(&mut self) -> PResult<(String, Span)> {
        match self.cur().tok.clone() {
            Tok::Ident(s) => {
                let sp = self.span();
                self.bump();
                Ok((s, sp))
            }
            _ => Err(self.error(format!(
                "expected identifier (found {})",
                self.cur().tok.describe()
            ))),
        }
    }

    // ---------------- declarations & statements ----------------
    fn declaration(&mut self) -> PResult<Stmt> {
        let sp = self.span();
        match &self.cur().tok {
            Tok::Auto => self.var_decl(VarKind::Auto),
            Tok::Const => self.var_decl(VarKind::Const),
            Tok::Own => self.var_decl(VarKind::Own),
            Tok::Func => {
                let f = self.func_decl(false)?;
                Ok(Stmt::new(StmtKind::Func(Rc::new(f)), sp))
            }
            Tok::StructKw => self.struct_decl(),
            Tok::ImplKw => self.impl_decl(),
            Tok::TraitKw => self.trait_decl(),
            Tok::Import => self.import_stmt(),
            Tok::LBrace => {
                self.bump();
                Ok(Stmt::new(StmtKind::Block(self.block_body()?), sp))
            }
            Tok::If => self.if_stmt(),
            Tok::While => self.while_stmt(),
            Tok::For => self.for_stmt(),
            Tok::Match => {
                let (subject, arms) = self.match_expr()?;
                Ok(Stmt::new(
                    StmtKind::MatchStmt {
                        subject: Box::new(subject),
                        arms,
                    },
                    sp,
                ))
            }
            Tok::Return => {
                self.bump();
                let value = if self.at(&Tok::Semi) {
                    None
                } else {
                    Some(Box::new(self.expression()?))
                };
                self.expect(&Tok::Semi, "';'")?;
                Ok(Stmt::new(StmtKind::Return(value), sp))
            }
            Tok::Break => {
                self.bump();
                self.expect(&Tok::Semi, "';'")?;
                Ok(Stmt::new(StmtKind::Break, sp))
            }
            Tok::Continue => {
                self.bump();
                self.expect(&Tok::Semi, "';'")?;
                Ok(Stmt::new(StmtKind::Continue, sp))
            }
            _ => {
                let e = self.expression()?;
                self.expect(&Tok::Semi, "';'")?;
                Ok(Stmt::new(StmtKind::ExprStmt(Box::new(e)), sp))
            }
        }
    }

    fn var_decl(&mut self, kind: VarKind) -> PResult<Stmt> {
        let sp = self.span();
        self.bump(); // auto/const/own
        let (name, _) = self.expect_ident()?;
        let ty = if self.eat(&Tok::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect(&Tok::Eq, "'=' in variable declaration")?;
        let value = self.expression()?;
        self.expect(&Tok::Semi, "';'")?;
        Ok(Stmt::new(
            StmtKind::Var {
                kind,
                name,
                value: Box::new(value),
                ty,
            },
            sp,
        ))
    }

    // ---------------- types (v0.3) ----------------
    fn parse_type(&mut self) -> PResult<TypeExpr> {
        let sp = self.span();
        let (name, _) = self.expect_ident()?;
        let mut args = Vec::new();
        if self.eat(&Tok::Lt) {
            loop {
                args.push(self.parse_type()?);
                if self.eat(&Tok::Comma) {
                    continue;
                }
                self.expect_gt()?;
                break;
            }
        }
        Ok(TypeExpr { name, args, span: sp })
    }

    /// expect `>`; `>>` (lexed as one Shl/Shr token: here Shr) counts as two
    fn expect_gt(&mut self) -> PResult<()> {
        if self.at(&Tok::Gt) {
            self.bump();
            return Ok(());
        }
        if self.at(&Tok::Shr) {
            // split: consume one `>`, leave the other as a Gt token
            let sp = self.span();
            self.toks[self.pos] = Token::new(Tok::Gt, sp.line, sp.col + 1);
            return Ok(());
        }
        Err(self.error(format!(
            "expected '>' to close type arguments (found {})",
            self.cur().tok.describe()
        )))
    }

    fn func_decl(&mut self, is_method: bool) -> PResult<FuncDef> {
        let sp = self.span();
        self.bump(); // func
        let (name, _) = self.expect_ident()?;
        let (params, receiver) = self.params(is_method)?;
        let ret_ty = if self.eat(&Tok::Arrow) {
            Some(self.parse_type()?)
        } else {
            None
        };
        let body = self.block()?;
        Ok(FuncDef {
            name,
            params,
            body,
            span: sp,
            ret_ty,
            receiver,
            is_method,
        })
    }

    // ---------------- OOP declarations (v0.3) ----------------
    fn struct_decl(&mut self) -> PResult<Stmt> {
        let sp = self.span();
        self.bump(); // struct
        let (name, _) = self.expect_ident()?;
        self.expect(&Tok::LBrace, "'{'")?;
        let mut fields = Vec::new();
        while !self.at(&Tok::RBrace) && !self.at(&Tok::Eof) {
            let fsp = self.span();
            let (fname, _) = self.expect_ident()?;
            let ty = if self.eat(&Tok::Colon) {
                Some(self.parse_type()?)
            } else {
                None
            };
            let default = if self.eat(&Tok::Eq) {
                Some(self.expression()?)
            } else {
                None
            };
            if default.is_some() && ty.is_none() {
                return Err(ParseError {
                    msg: format!(
                        "field \"{}\" with a default value needs a type (e.g. {}: int = ...)",
                        fname, fname
                    ),
                    span: fsp,
                });
            }
            fields.push(FieldDef {
                name: fname,
                ty,
                default,
                span: fsp,
            });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RBrace, "'}'")?;
        Ok(Stmt::new(StmtKind::Struct { name, fields }, sp))
    }

    fn impl_decl(&mut self) -> PResult<Stmt> {
        let sp = self.span();
        self.bump(); // impl
        let (first, _) = self.expect_ident()?;
        // impl Trait for Struct { ... }   |   impl Struct { ... }
        let (trait_name, target) = if self.eat(&Tok::For) {
            let (t, _) = self.expect_ident()?;
            (Some(first), t)
        } else {
            (None, first)
        };
        self.expect(&Tok::LBrace, "'{'")?;
        let mut methods: Vec<Rc<FuncDef>> = Vec::new();
        while !self.at(&Tok::RBrace) && !self.at(&Tok::Eof) {
            let msp = self.span();
            match self.cur().tok {
                Tok::Func => {
                    methods.push(Rc::new(self.func_decl(true)?));
                }
                _ => return Err(self.error("expected a method (func ...) in impl block")),
            }
            let _ = msp;
        }
        self.expect(&Tok::RBrace, "'}'")?;
        Ok(Stmt::new(
            StmtKind::Impl {
                target,
                trait_name,
                methods,
            },
            sp,
        ))
    }

    fn trait_decl(&mut self) -> PResult<Stmt> {
        let sp = self.span();
        self.bump(); // trait
        let (name, _) = self.expect_ident()?;
        self.expect(&Tok::LBrace, "'{'")?;
        let mut methods: Vec<Rc<FuncDef>> = Vec::new();
        while !self.at(&Tok::RBrace) && !self.at(&Tok::Eof) {
            self.expect(&Tok::Func, "'func' in trait block")?;
            let (mname, _) = self.expect_ident()?;
            let (params, receiver) = self.params(true)?;
            let ret_ty = if self.eat(&Tok::Arrow) {
                Some(self.parse_type()?)
            } else {
                None
            };
            // `;` = required method, `{ ... }` = default implementation
            let body = if self.eat(&Tok::Semi) {
                Vec::new()
            } else {
                self.block()?
            };
            let msp = sp;
            methods.push(Rc::new(FuncDef {
                name: mname,
                params,
                body,
                span: msp,
                ret_ty,
                receiver,
                is_method: true,
            }));
        }
        self.expect(&Tok::RBrace, "'}'")?;
        Ok(Stmt::new(StmtKind::Trait { name, methods }, sp))
    }

    fn params(&mut self, is_method: bool) -> PResult<(Vec<Param>, Option<Receiver>)> {
        self.expect(&Tok::LParen, "'('")?;
        let mut out = Vec::new();
        let mut receiver: Option<Receiver> = None;
        if self.at(&Tok::RParen) {
            self.bump();
            return Ok((out, receiver));
        }
        // method receiver as first parameter: self | &self | &mut self
        if is_method {
            let save = self.pos;
            let mut rec: Option<Receiver> = None;
            let mut by_amp = false;
            if self.at(&Tok::Amp) {
                self.bump();
                by_amp = true;
                let is_mut = self.eat(&Tok::Mut);
                if let Tok::Ident(n) = self.cur().tok.clone() {
                    if n == "self" {
                        self.bump();
                        rec = Some(if is_mut { Receiver::MutRef } else { Receiver::Ref });
                    }
                }
            } else if let Tok::Ident(n) = self.cur().tok.clone() {
                if n == "self" {
                    // bare `self` (or `self, args...`): in Plix bare self is
                    // sugar for `&self`
                    self.bump();
                    rec = Some(Receiver::Ref);
                }
            }
            let _ = by_amp;
            match rec {
                Some(r) => {
                    receiver = Some(r);
                    out.push(Param {
                        name: "self".to_string(),
                        default: None,
                        rest: false,
                        ty: None,
                    });
                    if self.at(&Tok::Comma) {
                        self.bump();
                    }
                    if self.at(&Tok::RParen) {
                        self.bump();
                        return Ok((out, receiver));
                    }
                }
                None => {
                    self.pos = save;
                }
            }
        }
        loop {
            let rest = if self.at(&Tok::DotDot) {
                // `...name`  (lexer gives DotDot + Dot)
                self.bump();
                self.expect(&Tok::Dot, "'.' to complete '...'")?;
                true
            } else {
                false
            };
            let (name, nsp) = self.expect_ident()?;
            let ty = if self.eat(&Tok::Colon) {
                Some(self.parse_type()?)
            } else {
                None
            };
            let default = if self.eat(&Tok::Eq) {
                Some(self.expression()?)
            } else {
                None
            };
            if default.is_some() && rest {
                return Err(ParseError {
                    msg: "rest parameter cannot have a default value".into(),
                    span: nsp,
                });
            }
            out.push(Param {
                name,
                default,
                rest,
                ty,
            });
            if rest {
                break;
            }
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        // rest must be last
        if let Some(p) = out.iter().position(|p| p.rest) {
            if p != out.len() - 1 {
                return Err(ParseError {
                    msg: "rest parameter must be the last parameter".into(),
                    span: self.span(),
                });
            }
        }
        // no parameters after the first default may lack one
        let mut seen_default = false;
        for p in &out {
            if p.rest || p.name == "self" && receiver.is_some() {
                continue;
            }
            if p.default.is_some() {
                seen_default = true;
            } else if seen_default {
                return Err(ParseError {
                    msg: format!(
                        "parameter \"{}\" needs a default value (defaults start here)",
                        p.name
                    ),
                    span: self.span(),
                });
            }
        }
        self.expect(&Tok::RParen, "')'")?;
        Ok((out, receiver))
    }

    fn block(&mut self) -> PResult<Vec<Stmt>> {
        self.expect(&Tok::LBrace, "'{'")?;
        self.block_body()
    }
    fn block_body(&mut self) -> PResult<Vec<Stmt>> {
        let mut out = Vec::new();
        while !self.at(&Tok::RBrace) && !self.at(&Tok::Eof) {
            out.push(self.declaration()?);
        }
        self.expect(&Tok::RBrace, "'}'")?;
        Ok(out)
    }

    fn statement(&mut self) -> PResult<Stmt> {
        self.declaration()
    }

    fn if_stmt(&mut self) -> PResult<Stmt> {
        let sp = self.span();
        self.bump(); // if
        self.expect(&Tok::LParen, "'('")?;
        let cond = self.expression()?;
        self.expect(&Tok::RParen, "')'")?;
        let then = self.statement()?;
        let els = if self.eat(&Tok::Else) {
            Some(Box::new(self.statement()?))
        } else {
            None
        };
        Ok(Stmt::new(
            StmtKind::If {
                cond: Box::new(cond),
                then: Box::new(then),
                els,
            },
            sp,
        ))
    }

    fn while_stmt(&mut self) -> PResult<Stmt> {
        let sp = self.span();
        self.bump(); // while
        self.expect(&Tok::LParen, "'('")?;
        let cond = self.expression()?;
        self.expect(&Tok::RParen, "')'")?;
        let body = self.statement()?;
        Ok(Stmt::new(
            StmtKind::While {
                cond: Box::new(cond),
                body: Box::new(body),
            },
            sp,
        ))
    }

    fn for_stmt(&mut self) -> PResult<Stmt> {
        let sp = self.span();
        self.bump(); // for
        self.expect(&Tok::LParen, "'('")?;

        // for-in:  for (x in expr)  |  for (auto x: int in expr)
        let save = self.pos;
        let mut forin_name: Option<String> = None;
        let mut forin_ty: Option<TypeExpr> = None;
        if self.at(&Tok::Auto) || self.at(&Tok::Own) {
            self.bump();
        }
        if let Tok::Ident(name) = self.cur().tok.clone() {
            self.bump();
            let mut tya: Option<TypeExpr> = None;
            let mut ok = true;
            if self.at(&Tok::Colon) {
                self.bump();
                match self.parse_type() {
                    Ok(t) => tya = Some(t),
                    Err(_) => ok = false,
                }
            }
            if ok && self.at(&Tok::In) {
                self.bump();
                forin_name = Some(name);
                forin_ty = tya;
            }
        }
        if let Some(name) = forin_name {
            let iter = self.expression()?;
            self.expect(&Tok::RParen, "')'")?;
            let body = self.statement()?;
            return Ok(Stmt::new(
                StmtKind::ForIn {
                    name,
                    iter: Box::new(iter),
                    body: Box::new(body),
                    ty: forin_ty,
                },
                sp,
            ));
        }
        self.pos = save; // backtrack: classic C-style for

        let init: Option<Box<Stmt>> = if self.eat(&Tok::Semi) {
            None
        } else if matches!(self.cur().tok, Tok::Auto | Tok::Const | Tok::Own) {
            let kind = match self.cur().tok {
                Tok::Auto => VarKind::Auto,
                Tok::Const => VarKind::Const,
                _ => VarKind::Own,
            };
            Some(Box::new(self.var_decl(kind)?))
        } else {
            let e = self.expression()?;
            self.expect(&Tok::Semi, "';'")?;
            Some(Box::new(Stmt::new(StmtKind::ExprStmt(Box::new(e)), sp)))
        };
        let cond = if self.at(&Tok::Semi) {
            None
        } else {
            Some(self.expression()?)
        };
        self.expect(&Tok::Semi, "';'")?;
        let step = if self.at(&Tok::RParen) {
            None
        } else {
            Some(self.expression()?)
        };
        self.expect(&Tok::RParen, "')'")?;
        let body = self.statement()?;
        Ok(Stmt::new(
            StmtKind::ForC {
                init,
                cond,
                step,
                body: Box::new(body),
            },
            sp,
        ))
    }

    fn import_stmt(&mut self) -> PResult<Stmt> {
        let sp = self.span();
        self.bump(); // import
        let python = self.eat(&Tok::PyKw);
        let (module, msp) = match self.cur().tok.clone() {
            Tok::Str(parts) => {
                if parts.iter().any(|p| matches!(p, StrPart::Expr(_))) {
                    return Err(ParseError {
                        msg: "module name cannot contain interpolation".into(),
                        span: sp,
                    });
                }
                let s = match &parts[0] {
                    StrPart::Lit(s) => s.clone(),
                    _ => unreachable!(),
                };
                self.bump();
                (s, sp)
            }
            _ => {
                return Err(self.error("expected module path string after import"));
            }
        };
        let _ = msp;
        let default_alias = module
            .rsplit('/')
            .next()
            .unwrap_or(&module)
            .trim_end_matches(".px")
            .to_string();
        let alias = if self.eat(&Tok::As) {
            let (a, _) = self.expect_ident()?;
            a
        } else {
            default_alias
        };
        self.expect(&Tok::Semi, "';'")?;
        Ok(Stmt::new(
            StmtKind::Import {
                module,
                alias,
                python,
            },
            sp,
        ))
    }

    // ---------------- match ----------------
    /// Parses `match <expr> { arms }` (parens optional) — used for both
    /// statement and expression position.
    fn match_expr(&mut self) -> PResult<(Expr, Vec<MatchArm>)> {
        let sp = self.span();
        self.bump(); // match
        let subject = if self.eat(&Tok::LParen) {
            let s = self.expression()?;
            self.expect(&Tok::RParen, "')'")?;
            s
        } else {
            // paren-less subject: `match x { ... }` — the '{ after x is the
            // arm block, never a struct literal
            let saved = self.no_struct_lit;
            self.no_struct_lit = true;
            let s = self.expression();
            self.no_struct_lit = saved;
            s?
        };
        self.expect(&Tok::LBrace, "'{'")?;
        let mut arms = Vec::new();
        while !self.at(&Tok::RBrace) && !self.at(&Tok::Eof) {
            let asp = self.span();
            let mut pats = vec![self.pattern()?];
            while self.at(&Tok::Pipe) {
                // 1 | 2 | 3 => ...
                if matches!(self.toks.get(self.pos + 1).map(|t| &t.tok), Some(Tok::Pipe)) {
                    break; // || operator cannot appear in patterns
                }
                self.bump();
                pats.push(self.pattern()?);
            }
            self.expect(&Tok::FatArrow, "'=>'")?;
            let body = if self.at(&Tok::LBrace) {
                self.bump();
                MatchBody::Block(self.block_body()?)
            } else {
                let e = self.expression()?;
                MatchBody::Expr(Box::new(e))
            };
            self.eat(&Tok::Comma);
            arms.push(MatchArm {
                pats,
                body,
                span: asp,
            });
        }
        self.expect(&Tok::RBrace, "'}'")?;
        Ok((Expr::new(ExprKind::Ident("<match-subject>".into()), sp), arms)
            .map_subject(subject))
    }

    fn pattern(&mut self) -> PResult<Pattern> {
        let t = self.cur().clone();
        match t.tok {
            Tok::NullKw => {
                self.bump();
                Ok(Pattern::Null)
            }
            Tok::True => {
                self.bump();
                Ok(Pattern::Bool(true))
            }
            Tok::False => {
                self.bump();
                Ok(Pattern::Bool(false))
            }
            Tok::Int(i) => {
                self.bump();
                Ok(Pattern::Int(i))
            }
            Tok::Float(f) => {
                self.bump();
                Ok(Pattern::Float(f))
            }
            Tok::Str(parts) => {
                if parts.len() == 1 {
                    if let StrPart::Lit(s) = &parts[0] {
                        let s = s.clone();
                        self.bump();
                        return Ok(Pattern::Str(s));
                    }
                }
                Err(ParseError {
                    msg: "pattern string cannot contain interpolation".into(),
                    span: t.span,
                })
            }
            Tok::Minus => {
                self.bump();
                match self.cur().tok {
                    Tok::Int(i) => {
                        self.bump();
                        Ok(Pattern::Int(-i))
                    }
                    Tok::Float(f) => {
                        self.bump();
                        Ok(Pattern::Float(-f))
                    }
                    _ => Err(self.error("expected number after '-' in pattern")),
                }
            }
            Tok::Ident(name) => {
                self.bump();
                if name == "_" {
                    Ok(Pattern::Wildcard)
                } else {
                    Ok(Pattern::Ident(name))
                }
            }
            _ => Err(self.error(format!(
                "expected pattern (found {})",
                t.tok.describe()
            ))),
        }
    }

    // ---------------- expressions ----------------
    fn expression(&mut self) -> PResult<Expr> {
        self.assignment()
    }

    fn assignment(&mut self) -> PResult<Expr> {
        let sp = self.span();
        let lhs = self.ternary()?;
        let op = match self.cur().tok {
            Tok::Eq => AssignOp::Eq,
            Tok::PlusEq => AssignOp::Add,
            Tok::MinusEq => AssignOp::Sub,
            Tok::StarEq => AssignOp::Mul,
            Tok::SlashEq => AssignOp::Div,
            Tok::PercentEq => AssignOp::Mod,
            _ => return Ok(lhs),
        };
        self.bump();
        let value = self.assignment()?; // right associative
        let target = match lhs.node {
            ExprKind::Ident(name) => AssignTarget::Ident(name),
            ExprKind::Index(o, i) => AssignTarget::Index(o, i),
            ExprKind::Member(o, m) => AssignTarget::Member(o, m),
            _ => {
                return Err(ParseError {
                    msg: "invalid assignment target".into(),
                    span: sp,
                })
            }
        };
        Ok(Expr::new(
            ExprKind::Assign {
                target,
                op,
                value: Box::new(value),
            },
            sp,
        ))
    }

    fn ternary(&mut self) -> PResult<Expr> {
        let sp = self.span();
        let c = self.logical_or()?;
        if self.eat(&Tok::Question) {
            let a = self.assignment()?;
            self.expect(&Tok::Colon, "':' in ternary")?;
            let b = self.ternary()?;
            return Ok(Expr::new(
                ExprKind::Ternary(Box::new(c), Box::new(a), Box::new(b)),
                sp,
            ));
        }
        Ok(c)
    }

    fn bin_level(
        &mut self,
        sub: fn(&mut Self) -> PResult<Expr>,
        ops: &[(Tok, BinOp)],
    ) -> PResult<Expr> {
        let mut lhs = sub(self)?;
        loop {
            let op = match ops.iter().find(|(t, _)| &self.cur().tok == t) {
                Some((_, o)) => *o,
                None => break,
            };
            let sp = self.span();
            self.bump();
            let rhs = sub(self)?;
            lhs = Expr::new(ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)), sp);
        }
        Ok(lhs)
    }

    fn logical_or(&mut self) -> PResult<Expr> {
        let mut lhs = self.logical_and()?;
        while self.at(&Tok::PipePipe) {
            let sp = self.span();
            self.bump();
            let rhs = self.logical_and()?;
            lhs = Expr::new(
                ExprKind::Logical(LogicalOp::Or, Box::new(lhs), Box::new(rhs)),
                sp,
            );
        }
        Ok(lhs)
    }
    fn logical_and(&mut self) -> PResult<Expr> {
        let mut lhs = self.bin_bitor()?;
        while self.at(&Tok::AmpAmp) {
            let sp = self.span();
            self.bump();
            let rhs = self.bin_bitor()?;
            lhs = Expr::new(
                ExprKind::Logical(LogicalOp::And, Box::new(lhs), Box::new(rhs)),
                sp,
            );
        }
        Ok(lhs)
    }
    fn bin_bitor(&mut self) -> PResult<Expr> {
        self.bin_level(Parser::bin_bitxor, &[(Tok::Pipe, BinOp::BOr)])
    }
    fn bin_bitxor(&mut self) -> PResult<Expr> {
        self.bin_level(Parser::bin_bitand, &[(Tok::Caret, BinOp::BXor)])
    }
    fn bin_bitand(&mut self) -> PResult<Expr> {
        self.bin_level(Parser::bin_equality, &[(Tok::Amp, BinOp::BAnd)])
    }
    fn bin_equality(&mut self) -> PResult<Expr> {
        self.bin_level(
            Parser::bin_comparison,
            &[(Tok::EqEq, BinOp::Eq), (Tok::BangEq, BinOp::Ne)],
        )
    }
    fn bin_comparison(&mut self) -> PResult<Expr> {
        self.bin_level(
            Parser::bin_shift,
            &[
                (Tok::Lt, BinOp::Lt),
                (Tok::LtEq, BinOp::Le),
                (Tok::Gt, BinOp::Gt),
                (Tok::GtEq, BinOp::Ge),
            ],
        )
    }
    fn bin_shift(&mut self) -> PResult<Expr> {
        self.bin_level(
            Parser::bin_term,
            &[(Tok::Shl, BinOp::Shl), (Tok::Shr, BinOp::Shr)],
        )
    }
    fn bin_term(&mut self) -> PResult<Expr> {
        self.bin_level(
            Parser::bin_factor,
            &[(Tok::Plus, BinOp::Add), (Tok::Minus, BinOp::Sub)],
        )
    }
    fn bin_factor(&mut self) -> PResult<Expr> {
        self.bin_level(
            Parser::unary,
            &[
                (Tok::Star, BinOp::Mul),
                (Tok::Slash, BinOp::Div),
                (Tok::Percent, BinOp::Mod),
            ],
        )
    }

    fn unary(&mut self) -> PResult<Expr> {
        let sp = self.span();
        match self.cur().tok {
            Tok::Bang => {
                self.bump();
                let e = self.unary()?;
                Ok(Expr::new(ExprKind::Unary(UnOp::Not, Box::new(e)), sp))
            }
            Tok::Minus => {
                self.bump();
                let e = self.unary()?;
                Ok(Expr::new(ExprKind::Unary(UnOp::Neg, Box::new(e)), sp))
            }
            Tok::Tilde => {
                self.bump();
                let e = self.unary()?;
                Ok(Expr::new(ExprKind::Unary(UnOp::BitNot, Box::new(e)), sp))
            }
            Tok::Amp => {
                // &x or &mut x — ownership borrow
                self.bump();
                let mutable = self.eat(&Tok::Mut);
                let e = self.unary()?;
                match &e.node {
                    ExprKind::Ident(_) | ExprKind::Index(..) | ExprKind::Member(..) => Ok(
                        Expr::new(
                            ExprKind::Borrow {
                                mutable,
                                expr: Box::new(e),
                            },
                            sp,
                        ),
                    ),
                    _ => Err(ParseError {
                        msg: "borrow operator & requires a variable, index, or member".into(),
                        span: sp,
                    }),
                }
            }
            _ => self.call(),
        }
    }

    fn call(&mut self) -> PResult<Expr> {
        let mut e = self.primary()?;
        loop {
            let sp = self.span();
            match self.cur().tok {
                Tok::LParen => {
                    self.bump();
                    let mut args = Vec::new();
                    if !self.at(&Tok::RParen) {
                        loop {
                            args.push(self.expression()?);
                            if !self.eat(&Tok::Comma) {
                                break;
                            }
                        }
                    }
                    self.expect(&Tok::RParen, "')'")?;
                    e = Expr::new(ExprKind::Call(Box::new(e), args), sp);
                }
                Tok::LBracket => {
                    self.bump();
                    // slice forms: [a..b] [a..] [..b] [..]
                    if self.at(&Tok::DotDot) {
                        self.bump();
                        let end = if self.at(&Tok::RBracket) {
                            None
                        } else {
                            Some(Box::new(self.expression()?))
                        };
                        self.expect(&Tok::RBracket, "']'")?;
                        e = Expr::new(
                            ExprKind::Slice {
                                obj: Box::new(e),
                                start: None,
                                end,
                            },
                            sp,
                        );
                        continue;
                    }
                    let first = self.expression()?;
                    if self.at(&Tok::DotDot) {
                        self.bump();
                        let end = if self.at(&Tok::RBracket) {
                            None
                        } else {
                            Some(Box::new(self.expression()?))
                        };
                        self.expect(&Tok::RBracket, "']'")?;
                        e = Expr::new(
                            ExprKind::Slice {
                                obj: Box::new(e),
                                start: Some(Box::new(first)),
                                end,
                            },
                            sp,
                        );
                    } else {
                        self.expect(&Tok::RBracket, "']'")?;
                        e = Expr::new(ExprKind::Index(Box::new(e), Box::new(first)), sp);
                    }
                }
                Tok::Dot => {
                    self.bump();
                    let (name, _) = self.expect_ident()?;
                    e = Expr::new(ExprKind::Member(Box::new(e), name), sp);
                }
                Tok::LBrace => {
                    // struct literal: Point { x: 1.0, y: 2.0 }  (also after
                    // generic args if the name resolved earlier — not yet)
                    let struct_name = match &e.node {
                        ExprKind::Ident(n) if !self.no_struct_lit => n.clone(),
                        _ => break,
                    };
                    self.bump();
                    let mut fields: Vec<(String, Expr)> = Vec::new();
                    if !self.at(&Tok::RBrace) {
                        loop {
                            let (fname, fspan) = self.expect_ident()?;
                            let value = if self.eat(&Tok::Colon) {
                                self.expression()?
                            } else {
                                // shorthand: Point { x } == Point { x: x }
                                Expr::new(ExprKind::Ident(fname.clone()), fspan)
                            };
                            fields.push((fname, value));
                            if !self.eat(&Tok::Comma) {
                                break;
                            }
                            if self.at(&Tok::RBrace) {
                                break; // trailing comma
                            }
                        }
                    }
                    self.expect(&Tok::RBrace, "'}' in struct literal")?;
                    e = Expr::new(ExprKind::StructLit { name: struct_name, fields }, sp);
                }
                _ => break,
            }
        }
        Ok(e)
    }

    fn primary(&mut self) -> PResult<Expr> {
        let t = self.cur().clone();
        let sp = t.span;
        match t.tok {
            Tok::Int(i) => {
                self.bump();
                Ok(Expr::new(ExprKind::Int(i), sp))
            }
            Tok::Float(f) => {
                self.bump();
                Ok(Expr::new(ExprKind::Float(f), sp))
            }
            Tok::True => {
                self.bump();
                Ok(Expr::new(ExprKind::Bool(true), sp))
            }
            Tok::False => {
                self.bump();
                Ok(Expr::new(ExprKind::Bool(false), sp))
            }
            Tok::NullKw => {
                self.bump();
                Ok(Expr::new(ExprKind::Null, sp))
            }
            Tok::Ident(name) => {
                self.bump();
                Ok(Expr::new(ExprKind::Ident(name), sp))
            }
            Tok::Str(parts) => {
                self.bump();
                self.interpolated_string(parts, sp)
            }
            Tok::LParen => {
                self.bump();
                let e = self.expression()?;
                self.expect(&Tok::RParen, "')'")?;
                Ok(e)
            }
            Tok::LBracket => {
                self.bump();
                let mut items = Vec::new();
                if !self.at(&Tok::RBracket) {
                    loop {
                        items.push(self.expression()?);
                        if !self.eat(&Tok::Comma) {
                            break;
                        }
                    }
                }
                self.expect(&Tok::RBracket, "']'")?;
                Ok(Expr::new(ExprKind::Array(items), sp))
            }
            Tok::LBrace => {
                self.bump();
                let mut props = Vec::new();
                if !self.at(&Tok::RBrace) {
                    loop {
                        let key = match self.cur().tok.clone() {
                            Tok::Ident(k) => {
                                self.bump();
                                k
                            }
                            Tok::Str(parts) => {
                                if parts.len() != 1 {
                                    return Err(self.error("invalid object key"));
                                }
                                match &parts[0] {
                                    StrPart::Lit(k) => {
                                        let k = k.clone();
                                        self.bump();
                                        k
                                    }
                                    _ => return Err(self.error("invalid object key")),
                                }
                            }
                            _ => return Err(self.error("expected object key")),
                        };
                        self.expect(&Tok::Colon, "':'")?;
                        let v = self.expression()?;
                        props.push((key, v));
                        if !self.eat(&Tok::Comma) {
                            break;
                        }
                    }
                }
                self.expect(&Tok::RBrace, "'}'")?;
                Ok(Expr::new(ExprKind::Object(props), sp))
            }
            Tok::Func => {
                // anonymous function literal (optionally named for recursion)
                self.bump();
                let name = if let Tok::Ident(n) = self.cur().tok.clone() {
                    self.bump();
                    n
                } else {
                    "<closure>".to_string()
                };
                let (params, _) = self.params(false)?;
                let ret_ty = if self.eat(&Tok::Arrow) {
                    Some(self.parse_type()?)
                } else {
                    None
                };
                let body = self.block()?;
                Ok(Expr::new(
                    ExprKind::FuncLit(Rc::new(FuncDef {
                        name,
                        params,
                        body,
                        span: sp,
                        ret_ty,
                        receiver: None,
                        is_method: false,
                    })),
                    sp,
                ))
            }
            Tok::Match => {
                let (subject, arms) = self.match_expr()?;
                Ok(Expr::new(
                    ExprKind::Match {
                        subject: Box::new(subject),
                        arms,
                    },
                    sp,
                ))
            }
            _ => Err(self.error(format!(
                "expected expression (found {})",
                t.tok.describe()
            ))),
        }
    }

    fn interpolated_string(&mut self, parts: Vec<StrPart>, sp: Span) -> PResult<Expr> {
        if parts.len() == 1 {
            if let StrPart::Lit(s) = &parts[0] {
                return Ok(Expr::new(ExprKind::Str(s.clone()), sp));
            }
        }
        let mut acc: Option<Expr> = None;
        for part in parts {
            let piece = match part {
                StrPart::Lit(s) => Expr::new(ExprKind::Str(s), sp),
                StrPart::Expr(src) => {
                    let e = parse_expr_source(&src).map_err(|pe| ParseError {
                        msg: format!("in interpolation ${{{}}}: {}", src, pe.msg),
                        span: sp,
                    })?;
                    // str(expr) for coercion
                    Expr::new(
                        ExprKind::Call(
                            Box::new(Expr::new(ExprKind::Ident("str".into()), sp)),
                            vec![e],
                        ),
                        sp,
                    )
                }
            };
            acc = Some(match acc {
                None => piece,
                Some(a) => Expr::new(ExprKind::Binary(BinOp::Add, Box::new(a), Box::new(piece)), sp),
            });
        }
        Ok(acc.unwrap())
    }
}

// helper to inject the real subject into the placeholder pair produced by
// match_expr (keeps the parser's borrow checker happy)
trait MapSubject {
    fn map_subject(self, subject: Expr) -> (Expr, Vec<MatchArm>);
}
impl MapSubject for (Expr, Vec<MatchArm>) {
    fn map_subject(self, subject: Expr) -> (Expr, Vec<MatchArm>) {
        (subject, self.1)
    }
}
