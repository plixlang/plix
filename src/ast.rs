//! Plix abstract syntax tree.

use crate::token::Span;
use std::rc::Rc;

#[derive(Debug, Clone)]
pub struct Node<T> {
    pub node: T,
    pub span: Span,
    /// checker annotations written on the AST node itself (interior
    /// mutability): survives as long as the node does, so the interpreter
    /// and the native backend can both consult it. Unused (0) on statements.
    pub flags: std::cell::Cell<u8>,
}

impl<T> Node<T> {
    pub fn new(node: T, span: Span) -> Node<T> {
        Node {
            node,
            span,
            flags: std::cell::Cell::new(0),
        }
    }
}

/// node flag: the checker proved this Add/Sub/Mul (or compound assignment)
/// operates on two `int`s. Typed int arithmetic is strict i64: overflow is
/// a RuntimeError instead of dynamic float promotion. Keeps the interpreter
/// and the native backend observably identical.
pub const FLAG_STRICT_INT_ARITH: u8 = 1;
/// node flags: the node holds a *typed slot* (annotated `int`/`float`/`bool`
/// variable declaration, for-in header, or assignment to a typed local).
/// The interpreter enforces the boundary guard / representation conversion
/// (int -> float widening, truthiness for bool) at exactly the points where
/// the native backend emits its unboxed-slot guards.
pub const FLAG_GUARD_INT: u8 = 1 << 1;
pub const FLAG_GUARD_FLOAT: u8 = 1 << 2;
pub const FLAG_GUARD_BOOL: u8 = 1 << 3;

pub type Expr = Node<ExprKind>;
pub type Stmt = Node<StmtKind>;

// ---------------------------------------------------------------------------
// expressions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    BAnd,
    BOr,
    BXor,
    Shl,
    Shr,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnOp {
    Neg,
    Not,
    BitNot,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LogicalOp {
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AssignOp {
    Eq,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

#[derive(Debug, Clone)]
pub enum AssignTarget {
    Ident(String),
    Index(Box<Expr>, Box<Expr>),
    Member(Box<Expr>, String),
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Ident(String),
    Array(Vec<Expr>),
    Object(Vec<(String, Expr)>),
    Unary(UnOp, Box<Expr>),
    /// `&x` / `&mut x` — ownership borrow (validated by the ownership checker)
    Borrow { mutable: bool, expr: Box<Expr> },
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Logical(LogicalOp, Box<Expr>, Box<Expr>),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    Assign { target: AssignTarget, op: AssignOp, value: Box<Expr> },
    Call(Box<Expr>, Vec<Expr>),
    Index(Box<Expr>, Box<Expr>),
    Slice { obj: Box<Expr>, start: Option<Box<Expr>>, end: Option<Box<Expr>> },
    Member(Box<Expr>, String),
    FuncLit(Rc<FuncDef>),
    /// match as an expression (arms produce values with `=> expr`)
    Match { subject: Box<Expr>, arms: Vec<MatchArm> },
    /// struct literal: `Point { x: 1.0, y: 2.0 }`
    StructLit { name: String, fields: Vec<(String, Expr)> },
}

// ---------------------------------------------------------------------------
// types (v0.3): annotations are optional everywhere; `any`/missing = dynamic
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct TypeExpr {
    pub name: String,
    pub args: Vec<TypeExpr>,
    pub span: crate::token::Span,
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pats: Vec<Pattern>,
    pub body: MatchBody,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum MatchBody {
    Expr(Box<Expr>),
    Block(Vec<Stmt>),
}

#[derive(Debug, Clone)]
pub enum Pattern {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    /// binds the subject into a new variable
    Ident(String),
    Wildcard,
}

// ---------------------------------------------------------------------------
// statements / declarations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarKind {
    /// `auto` — type-inferred, automatically memory-managed
    Auto,
    /// `const` — immutable binding
    Const,
    /// `own` — ownership semantics (static move/borrow checking)
    Own,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub default: Option<Expr>,
    /// `...rest` collects extra arguments into an array
    pub rest: bool,
    /// optional `name: type` annotation
    pub ty: Option<TypeExpr>,
}

/// Method receiver (`self` / `&self` / `&mut self` in an impl block).
/// `self` and `&self` are equivalent immutable borrows in Plix; the
/// receiver is always passed as the first parameter named "self".
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Receiver {
    Ref,
    MutRef,
}

#[derive(Debug)]
pub struct FuncDef {
    pub name: String,
    pub params: Vec<Param>,
    pub body: Vec<Stmt>,
    pub span: Span,
    /// `-> Type` return annotation
    pub ret_ty: Option<TypeExpr>,
    /// method receiver: Some(_) when defined inside an impl block with
    /// `func m(&self ...)` / `func m(&mut self ...)` / `func m(self ...)`
    pub receiver: Option<Receiver>,
    /// true when parsed inside an impl/trait block (methods never capture
    /// enclosing locals in native mode, like Rust)
    pub is_method: bool,
}

#[derive(Debug, Clone)]
pub struct FieldDef {
    pub name: String,
    pub ty: Option<TypeExpr>,
    pub default: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum StmtKind {
    Var { kind: VarKind, name: String, value: Box<Expr>, ty: Option<TypeExpr> },
    Func(Rc<FuncDef>),
    /// import "path.px" | import py "numpy" | import "fs"  (with optional `as`)
    Import { module: String, alias: String, python: bool },
    /// struct Point { x: float, y: float }
    Struct { name: String, fields: Vec<FieldDef> },
    /// impl Point { ... }  |  impl Shape for Point { ... }
    Impl { target: String, trait_name: Option<String>, methods: Vec<Rc<FuncDef>> },
    /// trait Shape { func area(&self) -> float;  func name(&self) { default } }
    Trait { name: String, methods: Vec<Rc<FuncDef>> },
    ExprStmt(Box<Expr>),
    Block(Vec<Stmt>),
    If { cond: Box<Expr>, then: Box<Stmt>, els: Option<Box<Stmt>> },
    While { cond: Box<Expr>, body: Box<Stmt> },
    /// C-style for
    ForC {
        init: Option<Box<Stmt>>,
        cond: Option<Expr>,
        step: Option<Expr>,
        body: Box<Stmt>,
    },
    ForIn { name: String, iter: Box<Expr>, body: Box<Stmt>, ty: Option<TypeExpr> },
    MatchStmt { subject: Box<Expr>, arms: Vec<MatchArm> },
    Return(Option<Box<Expr>>),
    Break,
    Continue,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct Program {
    pub stmts: Vec<Stmt>,
    pub source_name: String,
}

impl FuncDef {
    /// Stable identity for resolution tables.
    pub fn id(f: &Rc<FuncDef>) -> usize {
        Rc::as_ptr(f) as usize
    }
}
