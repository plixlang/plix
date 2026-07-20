//! Plix tokens.

#[derive(Debug, Clone, PartialEq)]
pub enum StrPart {
    Lit(String),
    /// Source text of an interpolation expression `${...}` (re-lexed later).
    Expr(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Int(i64),
    Float(f64),
    /// String literal, split into literal parts and interpolation exprs.
    Str(Vec<StrPart>),
    Ident(String),

    // keywords
    Auto,
    Const,
    Own,
    Func,
    Return,
    If,
    Else,
    For,
    While,
    Break,
    Continue,
    True,
    False,
    NullKw,
    Import,
    As,
    PyKw, // "py" is contextual: only special after `import`
    Match,
    In,
    Mut,
    StructKw,
    ImplKw,
    TraitKw,
    EnumKw,

    // punctuation
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Colon,
    Semi,
    Dot,
    DotDot,
    Question,
    FatArrow,
    Arrow, // ->

    // operators
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Amp,
    Pipe,
    Caret,
    Tilde,
    Shl,
    Shr,
    AmpAmp,
    PipePipe,
    Bang,
    Eq,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    EqEq,
    BangEq,
    Lt,
    LtEq,
    Gt,
    GtEq,

    Eof,
}

impl Tok {
    pub fn describe(&self) -> String {
        use Tok::*;
        match self {
            Int(i) => format!("int {}", i),
            Float(f) => format!("float {}", f),
            Str(_) => "string".into(),
            Ident(s) => format!("identifier \"{}\"", s),
            Eof => "end of file".into(),
            other => format!("{:?}", other),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

impl Token {
    pub fn new(tok: Tok, line: u32, col: u32) -> Token {
        Token {
            tok,
            span: Span { line, col },
        }
    }
}

pub fn keyword(s: &str) -> Option<Tok> {
    Some(match s {
        "auto" => Tok::Auto,
        "const" => Tok::Const,
        "own" => Tok::Own,
        "func" => Tok::Func,
        "return" => Tok::Return,
        "if" => Tok::If,
        "else" => Tok::Else,
        "for" => Tok::For,
        "while" => Tok::While,
        "break" => Tok::Break,
        "continue" => Tok::Continue,
        "true" => Tok::True,
        "false" => Tok::False,
        "null" | "None" => Tok::NullKw,
        "import" => Tok::Import,
        "as" => Tok::As,
        "py" => Tok::PyKw,
        "match" => Tok::Match,
        "in" => Tok::In,
        "mut" => Tok::Mut,
        "struct" => Tok::StructKw,
        "impl" => Tok::ImplKw,
        "trait" => Tok::TraitKw,
        "enum" => Tok::EnumKw,
        _ => return None,
    })
}
