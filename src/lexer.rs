//! Plix lexer.
//!
//! Numbers: decimal, hex (0x), binary (0b), octal (0o), underscores as
//! separators, floats with optional exponent.
//! Strings: "..." with escapes (\n \t \r \\ \" \' \$ \xNN) and interpolation
//! ${expr}; r"..." raw strings (no escapes, no interpolation).
//! Comments: // line and /* ... */ block.

use crate::token::{keyword, StrPart, Tok, Token};

pub struct LexError {
    pub msg: String,
    pub line: u32,
    pub col: u32,
}

impl LexError {
    fn new(msg: impl Into<String>, line: u32, col: u32) -> LexError {
        LexError {
            msg: msg.into(),
            line,
            col,
        }
    }
}

pub struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    line: u32,
    col: u32,
}

pub fn lex(src: &str) -> Result<Vec<Token>, LexError> {
    let mut lx = Lexer {
        src: src.as_bytes(),
        pos: 0,
        line: 1,
        col: 1,
    };
    let mut out = Vec::new();
    loop {
        let t = lx.next_token()?;
        let eof = t.tok == Tok::Eof;
        out.push(t);
        if eof {
            return Ok(out);
        }
    }
}

impl<'a> Lexer<'a> {
    fn peek(&self) -> u8 {
        *self.src.get(self.pos).unwrap_or(&0)
    }
    fn peek2(&self) -> u8 {
        *self.src.get(self.pos + 1).unwrap_or(&0)
    }
    fn peek3(&self) -> u8 {
        *self.src.get(self.pos + 2).unwrap_or(&0)
    }
    fn bump(&mut self) -> u8 {
        let c = self.peek();
        if c == b'\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        self.pos += 1;
        c
    }
    fn err<T>(&self, msg: impl Into<String>) -> Result<T, LexError> {
        Err(LexError::new(msg, self.line, self.col))
    }

    fn next_token(&mut self) -> Result<Token, LexError> {
        self.skip_ws_comments()?;
        let (line, col) = (self.line, self.col);
        let c = self.peek();
        if c == 0 {
            return Ok(Token::new(Tok::Eof, line, col));
        }
        let tok = match c {
            b'0'..=b'9' => return self.number(),
            b'"' => return self.string(false),
            b'r' if self.peek2() == b'"' => {
                self.bump();
                return self.string(true);
            }
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => return Ok(self.ident_or_keyword()),
            b'(' => {
                self.bump();
                Tok::LParen
            }
            b')' => {
                self.bump();
                Tok::RParen
            }
            b'{' => {
                self.bump();
                Tok::LBrace
            }
            b'}' => {
                self.bump();
                Tok::RBrace
            }
            b'[' => {
                self.bump();
                Tok::LBracket
            }
            b']' => {
                self.bump();
                Tok::RBracket
            }
            b',' => {
                self.bump();
                Tok::Comma
            }
            b':' => {
                self.bump();
                Tok::Colon
            }
            b';' => {
                self.bump();
                Tok::Semi
            }
            b'?' => {
                self.bump();
                Tok::Question
            }
            b'.' => {
                self.bump();
                if self.peek() == b'.' {
                    self.bump();
                    Tok::DotDot
                } else {
                    Tok::Dot
                }
            }
            b'=' => {
                self.bump();
                if self.peek() == b'=' {
                    self.bump();
                    Tok::EqEq
                } else if self.peek() == b'>' {
                    self.bump();
                    Tok::FatArrow
                } else {
                    Tok::Eq
                }
            }
            b'+' => {
                self.bump();
                if self.peek() == b'=' {
                    self.bump();
                    Tok::PlusEq
                } else {
                    Tok::Plus
                }
            }
            b'-' => {
                self.bump();
                if self.peek() == b'=' {
                    self.bump();
                    Tok::MinusEq
                } else if self.peek() == b'>' {
                    self.bump();
                    Tok::Arrow
                } else {
                    Tok::Minus
                }
            }
            b'*' => {
                self.bump();
                if self.peek() == b'=' {
                    self.bump();
                    Tok::StarEq
                } else {
                    Tok::Star
                }
            }
            b'/' => {
                self.bump();
                if self.peek() == b'=' {
                    self.bump();
                    Tok::SlashEq
                } else {
                    Tok::Slash
                }
            }
            b'%' => {
                self.bump();
                if self.peek() == b'=' {
                    self.bump();
                    Tok::PercentEq
                } else {
                    Tok::Percent
                }
            }
            b'!' => {
                self.bump();
                if self.peek() == b'=' {
                    self.bump();
                    Tok::BangEq
                } else {
                    Tok::Bang
                }
            }
            b'<' => {
                self.bump();
                if self.peek() == b'=' {
                    self.bump();
                    Tok::LtEq
                } else if self.peek() == b'<' {
                    self.bump();
                    Tok::Shl
                } else {
                    Tok::Lt
                }
            }
            b'>' => {
                self.bump();
                if self.peek() == b'=' {
                    self.bump();
                    Tok::GtEq
                } else if self.peek() == b'>' {
                    self.bump();
                    Tok::Shr
                } else {
                    Tok::Gt
                }
            }
            b'&' => {
                self.bump();
                if self.peek() == b'&' {
                    self.bump();
                    Tok::AmpAmp
                } else {
                    Tok::Amp
                }
            }
            b'|' => {
                self.bump();
                if self.peek() == b'|' {
                    self.bump();
                    Tok::PipePipe
                } else {
                    Tok::Pipe
                }
            }
            b'^' => {
                self.bump();
                Tok::Caret
            }
            b'~' => {
                self.bump();
                Tok::Tilde
            }
            _ => return self.err(format!("unexpected character {:?}", c as char)),
        };
        Ok(Token::new(tok, line, col))
    }

    fn skip_ws_comments(&mut self) -> Result<(), LexError> {
        loop {
            match self.peek() {
                b' ' | b'\t' | b'\r' | b'\n' => {
                    self.bump();
                }
                b'/' if self.peek2() == b'/' => {
                    while self.peek() != b'\n' && self.peek() != 0 {
                        self.bump();
                    }
                }
                b'/' if self.peek2() == b'*' => {
                    let (l, c) = (self.line, self.col);
                    self.bump();
                    self.bump();
                    loop {
                        if self.peek() == 0 {
                            return Err(LexError::new("unterminated block comment", l, c));
                        }
                        if self.peek() == b'*' && self.peek2() == b'/' {
                            self.bump();
                            self.bump();
                            break;
                        }
                        self.bump();
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    fn number(&mut self) -> Result<Token, LexError> {
        let (line, col) = (self.line, self.col);
        let start = self.pos;
        if self.peek() == b'0' && matches!(self.peek2(), b'x' | b'b' | b'o') {
            let base_ch = self.peek2();
            self.bump();
            self.bump();
            let ds = self.pos;
            let base = match base_ch {
                b'x' => 16,
                b'b' => 2,
                _ => 8,
            };
            while self.peek().is_ascii_alphanumeric() || self.peek() == b'_' {
                self.bump();
            }
            let text: String = std::str::from_utf8(&self.src[ds..self.pos])
                .unwrap()
                .chars()
                .filter(|&c| c != '_')
                .collect();
            return match i64::from_str_radix(&text, base) {
                Ok(v) => Ok(Token::new(Tok::Int(v), line, col)),
                Err(_) => self.err(format!("invalid {} literal", base_ch as char)),
            };
        }
        while self.peek().is_ascii_digit() || self.peek() == b'_' {
            self.bump();
        }
        let mut is_float = false;
        if self.peek() == b'.' && self.peek2().is_ascii_digit() {
            is_float = true;
            self.bump();
            while self.peek().is_ascii_digit() || self.peek() == b'_' {
                self.bump();
            }
        }
        if matches!(self.peek(), b'e' | b'E')
            && (self.peek2().is_ascii_digit()
                || (matches!(self.peek2(), b'+' | b'-') && self.peek3().is_ascii_digit()))
        {
            is_float = true;
            self.bump();
            if matches!(self.peek(), b'+' | b'-') {
                self.bump();
            }
            while self.peek().is_ascii_digit() {
                self.bump();
            }
        }
        let text: String = std::str::from_utf8(&self.src[start..self.pos])
            .unwrap()
            .chars()
            .filter(|&c| c != '_')
            .collect();
        if is_float {
            match text.parse::<f64>() {
                Ok(v) => Ok(Token::new(Tok::Float(v), line, col)),
                Err(_) => self.err("invalid float literal"),
            }
        } else {
            match text.parse::<i64>() {
                Ok(v) => Ok(Token::new(Tok::Int(v), line, col)),
                Err(_) => self.err("invalid integer literal (out of range)"),
            }
        }
    }

    fn ident_or_keyword(&mut self) -> Token {
        let (line, col) = (self.line, self.col);
        let start = self.pos;
        loop {
            let c = self.peek();
            if c.is_ascii_alphanumeric() || c == b'_' {
                self.bump();
            } else {
                break;
            }
        }
        let text = std::str::from_utf8(&self.src[start..self.pos]).unwrap();
        let tok = keyword(text).unwrap_or_else(|| Tok::Ident(text.to_string()));
        Token::new(tok, line, col)
    }

    fn string(&mut self, raw: bool) -> Result<Token, LexError> {
        let (line, col) = (self.line, self.col);
        let quote = self.bump(); // "
        let mut parts: Vec<StrPart> = Vec::new();
        let mut cur = String::new();
        loop {
            let c = self.peek();
            match c {
                0 => return self.err("unterminated string"),
                b'\n' => return self.err("newline in string literal (use \\n)"),
                b'"' if c == quote => {
                    self.bump();
                    break;
                }
                b'\\' if !raw => {
                    self.bump();
                    let e = self.peek();
                    self.bump();
                    match e {
                        b'n' => cur.push('\n'),
                        b't' => cur.push('\t'),
                        b'r' => cur.push('\r'),
                        b'\\' => cur.push('\\'),
                        b'"' => cur.push('"'),
                        b'\'' => cur.push('\''),
                        b'$' => cur.push('$'),
                        b'0' => cur.push('\0'),
                        b'x' => {
                            let h1 = self.bump();
                            let h2 = self.bump();
                            let s = format!("{}{}", h1 as char, h2 as char);
                            let v = u8::from_str_radix(&s, 16)
                                .map_err(|_| LexError::new("bad \\xNN escape", self.line, self.col))?;
                            cur.push(v as char);
                        }
                        _ => return self.err(format!("unknown escape \\{}", e as char)),
                    }
                }
                b'$' if !raw && self.peek2() == b'{' => {
                    self.bump();
                    self.bump();
                    if !cur.is_empty() {
                        parts.push(StrPart::Lit(std::mem::take(&mut cur)));
                    }
                    let mut depth = 1usize;
                    let mut expr = String::new();
                    loop {
                        let c = self.peek();
                        if c == 0 {
                            return self.err("unterminated ${...} interpolation");
                        }
                        if c == b'\\' {
                            // escape at the *outer* string level: `\x` keeps just
                            // the char (in particular \" does not open an inner
                            // string and does not affect brace depth)
                            self.bump();
                            let e = self.bump();
                            expr.push(e as char);
                            continue;
                        }
                        if c == b'"' {
                            // allow strings inside interpolation
                            expr.push(self.bump() as char);
                            while self.peek() != b'"' && self.peek() != 0 && self.peek() != b'\n' {
                                if self.peek() == b'\\' {
                                    expr.push(self.bump() as char);
                                }
                                if self.peek() != 0 {
                                    expr.push(self.bump() as char);
                                }
                            }
                            if self.peek() == b'"' {
                                expr.push(self.bump() as char);
                            }
                            continue;
                        }
                        if c == b'{' {
                            depth += 1;
                        }
                        if c == b'}' {
                            depth -= 1;
                            if depth == 0 {
                                self.bump();
                                break;
                            }
                        }
                        let utf = self.bump();
                        expr.push(utf as char);
                    }
                    parts.push(StrPart::Expr(expr.trim().to_string()));
                }
                b if b < 0x80 => {
                    cur.push(self.bump() as char);
                }
                _ => {
                    // keep raw UTF-8 bytes
                    let start = self.pos;
                    self.bump();
                    while self.peek() & 0xC0 == 0x80 {
                        self.bump();
                    }
                    cur.push_str(std::str::from_utf8(&self.src[start..self.pos]).unwrap_or(""));
                }
            }
        }
        if !cur.is_empty() || parts.is_empty() {
            parts.push(StrPart::Lit(cur));
        }
        Ok(Token::new(Tok::Str(parts), line, col))
    }
}
