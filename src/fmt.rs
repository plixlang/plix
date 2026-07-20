use crate::lexer;
use crate::parser;
use crate::token::{StrPart, Tok};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct FormatResult {
    pub changed: bool,
    pub formatted: String,
}

pub fn format_source(src: &str) -> Result<FormatResult, String> {
    // A formatter must never format syntactically-invalid code.
    parser::parse_file(src)
        .map_err(|e| format!("{}:{}: syntax error: {}", e.span.line, e.span.col, e.msg))?;
    let toks =
        lexer::lex(src).map_err(|e| format!("{}:{}: lex error: {}", e.line, e.col, e.msg))?;
    let mut out = String::new();
    let mut indent = 0usize;
    let mut line_start = true;
    let mut prev_word = false;
    let mut prev_tok: Option<Tok> = None;

    for t in toks {
        if matches!(t.tok, Tok::Eof) {
            break;
        }
        match &t.tok {
            Tok::RBrace | Tok::RBracket | Tok::RParen if line_start => {
                indent = indent.saturating_sub(1);
            }
            _ => {}
        }
        if line_start {
            out.push_str(&"    ".repeat(indent));
            line_start = false;
        }
        let word = is_word(&t.tok);
        if needs_space(prev_tok.as_ref(), &t.tok, prev_word, word) {
            out.push(' ');
        }
        out.push_str(&tok_text(&t.tok));
        match &t.tok {
            Tok::LBrace => {
                indent += 1;
                newline(&mut out, &mut line_start);
            }
            Tok::RBrace => {
                newline(&mut out, &mut line_start);
            }
            Tok::Semi => newline(&mut out, &mut line_start),
            Tok::Comma => out.push(' '),
            Tok::FatArrow => out.push(' '),
            _ => {}
        }
        prev_word = word;
        prev_tok = Some(t.tok);
    }
    while out.contains(" \n") {
        out = out.replace(" \n", "\n");
    }
    let mut cleaned = String::new();
    for line in out.lines() {
        cleaned.push_str(line.trim_end());
        cleaned.push('\n');
    }
    let formatted = cleaned;
    Ok(FormatResult {
        changed: normalized(src) != normalized(&formatted),
        formatted,
    })
}

pub fn collect_px_files(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_file() {
        if path.extension().map(|e| e == "px").unwrap_or(false) {
            out.push(path.to_path_buf());
        }
        return;
    }
    let Ok(rd) = std::fs::read_dir(path) else {
        return;
    };
    for ent in rd.flatten() {
        let p = ent.path();
        if p.is_dir() {
            match p.file_name().and_then(|n| n.to_str()) {
                Some("target" | ".git" | "node_modules" | "examples_bin") => continue,
                _ => {}
            }
            collect_px_files(&p, out);
        } else if p.extension().map(|e| e == "px").unwrap_or(false) {
            out.push(p);
        }
    }
}

fn normalized(s: &str) -> String {
    let mut x = s.replace("\r\n", "\n");
    if !x.ends_with('\n') {
        x.push('\n');
    }
    x
}

fn newline(out: &mut String, line_start: &mut bool) {
    while out.ends_with(' ') {
        out.pop();
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    *line_start = true;
}

fn is_word(t: &Tok) -> bool {
    matches!(
        t,
        Tok::Ident(_)
            | Tok::Int(_)
            | Tok::Float(_)
            | Tok::Str(_)
            | Tok::True
            | Tok::False
            | Tok::NullKw
    )
}

fn needs_space(prev: Option<&Tok>, cur: &Tok, prev_word: bool, cur_word: bool) -> bool {
    let Some(p) = prev else { return false };
    if prev_word && cur_word {
        return true;
    }
    if matches!(cur, Tok::LBrace) && !matches!(p, Tok::LBrace | Tok::LParen | Tok::LBracket) {
        return true;
    }
    matches!(
        (p, cur),
        (
            Tok::Auto
                | Tok::Const
                | Tok::Own
                | Tok::Func
                | Tok::Return
                | Tok::If
                | Tok::While
                | Tok::For
                | Tok::Match
                | Tok::Import
                | Tok::As
                | Tok::In
                | Tok::StructKw
                | Tok::EnumKw
                | Tok::ImplKw
                | Tok::TraitKw,
            _
        ) | (
            _,
            Tok::Eq
                | Tok::PlusEq
                | Tok::MinusEq
                | Tok::StarEq
                | Tok::SlashEq
                | Tok::PercentEq
                | Tok::EqEq
                | Tok::BangEq
                | Tok::Lt
                | Tok::LtEq
                | Tok::Gt
                | Tok::GtEq
                | Tok::Plus
                | Tok::Minus
                | Tok::Star
                | Tok::Slash
                | Tok::Percent
                | Tok::AmpAmp
                | Tok::PipePipe
                | Tok::FatArrow
                | Tok::Arrow
        ) | (
            Tok::Eq
                | Tok::PlusEq
                | Tok::MinusEq
                | Tok::StarEq
                | Tok::SlashEq
                | Tok::PercentEq
                | Tok::EqEq
                | Tok::BangEq
                | Tok::Lt
                | Tok::LtEq
                | Tok::Gt
                | Tok::GtEq
                | Tok::Plus
                | Tok::Minus
                | Tok::Star
                | Tok::Slash
                | Tok::Percent
                | Tok::AmpAmp
                | Tok::PipePipe
                | Tok::FatArrow
                | Tok::Arrow,
            _
        )
    ) && !matches!(
        cur,
        Tok::Comma | Tok::Semi | Tok::RParen | Tok::RBracket | Tok::RBrace
    )
}

fn tok_text(t: &Tok) -> String {
    match t {
        Tok::Int(i) => i.to_string(),
        Tok::Float(f) => f.to_string(),
        Tok::Str(parts) => {
            let mut s = String::from("\"");
            for p in parts {
                match p {
                    StrPart::Lit(x) => s.push_str(&x.replace('"', "\\\"")),
                    StrPart::Expr(e) => {
                        s.push_str("${");
                        s.push_str(e);
                        s.push('}');
                    }
                }
            }
            s.push('"');
            s
        }
        Tok::Ident(s) => s.clone(),
        Tok::Auto => "auto".into(),
        Tok::Const => "const".into(),
        Tok::Own => "own".into(),
        Tok::Func => "func".into(),
        Tok::Return => "return".into(),
        Tok::If => "if".into(),
        Tok::Else => "else".into(),
        Tok::For => "for".into(),
        Tok::While => "while".into(),
        Tok::Break => "break".into(),
        Tok::Continue => "continue".into(),
        Tok::True => "true".into(),
        Tok::False => "false".into(),
        Tok::NullKw => "None".into(),
        Tok::Import => "import".into(),
        Tok::As => "as".into(),
        Tok::PyKw => "py".into(),
        Tok::Match => "match".into(),
        Tok::In => "in".into(),
        Tok::Mut => "mut".into(),
        Tok::StructKw => "struct".into(),
        Tok::ImplKw => "impl".into(),
        Tok::TraitKw => "trait".into(),
        Tok::EnumKw => "enum".into(),
        Tok::LParen => "(".into(),
        Tok::RParen => ")".into(),
        Tok::LBrace => "{".into(),
        Tok::RBrace => "}".into(),
        Tok::LBracket => "[".into(),
        Tok::RBracket => "]".into(),
        Tok::Comma => ",".into(),
        Tok::Colon => ":".into(),
        Tok::Semi => ";".into(),
        Tok::Dot => ".".into(),
        Tok::DotDot => "..".into(),
        Tok::Question => "?".into(),
        Tok::FatArrow => "=>".into(),
        Tok::Arrow => "->".into(),
        Tok::Plus => "+".into(),
        Tok::Minus => "-".into(),
        Tok::Star => "*".into(),
        Tok::Slash => "/".into(),
        Tok::Percent => "%".into(),
        Tok::Amp => "&".into(),
        Tok::Pipe => "|".into(),
        Tok::Caret => "^".into(),
        Tok::Tilde => "~".into(),
        Tok::Shl => "<<".into(),
        Tok::Shr => ">>".into(),
        Tok::AmpAmp => "&&".into(),
        Tok::PipePipe => "||".into(),
        Tok::Bang => "!".into(),
        Tok::Eq => "=".into(),
        Tok::PlusEq => "+=".into(),
        Tok::MinusEq => "-=".into(),
        Tok::StarEq => "*=".into(),
        Tok::SlashEq => "/=".into(),
        Tok::PercentEq => "%=".into(),
        Tok::EqEq => "==".into(),
        Tok::BangEq => "!=".into(),
        Tok::Lt => "<".into(),
        Tok::LtEq => "<=".into(),
        Tok::Gt => ">".into(),
        Tok::GtEq => ">=".into(),
        Tok::Eof => String::new(),
    }
}
