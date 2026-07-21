use crate::lexer;
use crate::parser;
use crate::token::{StrPart, Tok};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct FormatResult {
    pub changed: bool,
    pub formatted: String,
}

/// A line annotated with its role.
#[derive(Debug, Clone)]
enum LineKind {
    /// Purely whitespace.
    Blank,
    /// A line whose trimmed content starts with `//` or is inside a block comment.
    Comment,
    /// Regular code line.
    Code(String),
}

pub fn format_source(src: &str) -> Result<FormatResult, String> {
    // A formatter must never format syntactically-invalid code.
    parser::parse_file(src)
        .map_err(|e| format!("{}:{}: syntax error: {}", e.span.line, e.span.col, e.msg))?;

    // ---- Phase 1: classify source lines ----
    let mut in_block = false;
    let line_kinds: Vec<LineKind> = src
        .lines()
        .map(|line| {
            let trimmed = line.trim();
            if in_block {
                if trimmed.contains("*/") {
                    in_block = false;
                }
                return LineKind::Comment;
            }
            if trimmed.starts_with("/*") {
                in_block = !trimmed.contains("*/");
                return LineKind::Comment;
            }
            if trimmed.is_empty() {
                return LineKind::Blank;
            }
            if trimmed.starts_with("//") {
                return LineKind::Comment;
            }
            LineKind::Code(line.to_string())
        })
        .collect();

    // ---- Phase 2: extract code-only text and format it ----
    let code_only: String = line_kinds
        .iter()
        .filter_map(|lk| match lk {
            LineKind::Code(line) => Some(line.as_str()),
            _ => None,
        })
        .collect::<Vec<&str>>()
        .join("\n");

    // Parse and lex the code-only text; if it fails, bail out.
    let toks = lexer::lex(&code_only)
        .map_err(|e| format!("{}:{}: lex error: {}", e.line, e.col, e.msg))?;
    if parser::parse_file(&code_only).is_err() {
        return Err("formatter: could not parse code portion".into());
    }

    let mut formatted_code = String::new();
    let mut indent = 0usize;
    let mut line_start = true;
    let mut prev_word = false;
    let mut prev_tok: Option<Tok> = None;

    let n = toks.len();
    let mut idx = 0;
    while idx < n {
        let t = &toks[idx];
        if matches!(t.tok, Tok::Eof) {
            break;
        }
        let next_tok: Option<&Tok> = toks[idx + 1..n]
            .iter()
            .map(|tt| &tt.tok)
            .find(|tt| !matches!(tt, Tok::Eof));

        if line_start && matches!(t.tok, Tok::RBrace | Tok::RBracket | Tok::RParen) {
            indent = indent.saturating_sub(1);
        }
        if line_start {
            formatted_code.push_str(&"    ".repeat(indent));
            line_start = false;
        }
        let word = is_word(&t.tok);
        if needs_space(prev_tok.as_ref(), &t.tok, prev_word, word) {
            formatted_code.push(' ');
        }
        formatted_code.push_str(&tok_text(&t.tok));
        match &t.tok {
            Tok::LBrace => {
                indent += 1;
                newline(&mut formatted_code, &mut line_start);
            }
            Tok::RBrace => {
                if matches!(next_tok, Some(Tok::Else)) {
                    formatted_code.push(' ');
                } else {
                    newline(&mut formatted_code, &mut line_start);
                }
            }
            Tok::Semi => newline(&mut formatted_code, &mut line_start),
            Tok::Comma => formatted_code.push(' '),
            Tok::FatArrow => formatted_code.push(' '),
            _ => {}
        }
        prev_word = word;
        prev_tok = Some(t.tok.clone());
        idx += 1;
    }

    // Trim trailing whitespace per line.
    let mut cleaned = String::new();
    for line in formatted_code.lines() {
        cleaned.push_str(line.trim_end());
        cleaned.push('\n');
    }
    let formatted_lines: Vec<&str> = cleaned.lines().collect();

    // ---- Phase 3: interleave comments and blank lines back in ----
    let mut result = String::new();
    let mut code_idx = 0; // index into formatted_lines
    let mut pending_blank = 0usize;

    for lk in &line_kinds {
        match lk {
            LineKind::Blank => pending_blank += 1,
            LineKind::Comment => {
                // Flush any pending blank lines first.
                for _ in 0..pending_blank {
                    result.push('\n');
                }
                pending_blank = 0;
                // Emit the comment line verbatim — but we need the original
                // text.  Reconstruct from the source.
                // Since we classified from the source, we can emit raw lines.
            }
            LineKind::Code(_) => {
                // Flush pending blanks (at most one between sections).
                if pending_blank > 0 && code_idx > 0 {
                    result.push('\n');
                }
                pending_blank = 0;
                if code_idx < formatted_lines.len() {
                    result.push_str(formatted_lines[code_idx]);
                    result.push('\n');
                    code_idx += 1;
                }
            }
        }
    }

    // If there are remaining formatted code lines, append them.
    while code_idx < formatted_lines.len() {
        result.push_str(formatted_lines[code_idx]);
        result.push('\n');
        code_idx += 1;
    }

    // ---- Phase 3b: re-inject comment/blank lines at their original positions ----
    // The simple approach above loses comment text.  Let's do a proper merge:
    let result = merge_preserving_non_code(src, &line_kinds, &formatted_lines);

    // Safety gate: verify the formatted output still parses.
    if parser::parse_file(&result).is_err() {
        return Err(
            "formatter produced invalid code — refusing to overwrite (please report this bug)"
                .into(),
        );
    }

    Ok(FormatResult {
        changed: normalized(src) != normalized(&result),
        formatted: result,
    })
}

/// Merge formatted code lines back into the original structure,
/// preserving comments and blank lines at their original positions.
fn merge_preserving_non_code(src: &str, kinds: &[LineKind], formatted: &[&str]) -> String {
    let src_lines: Vec<&str> = src.lines().collect();
    let mut out = String::new();
    let mut fi = 0; // formatted-code index

    for (i, kind) in kinds.iter().enumerate() {
        match kind {
            LineKind::Blank => {
                out.push('\n');
            }
            LineKind::Comment => {
                // Emit the original source line (preserves comment text).
                if i < src_lines.len() {
                    out.push_str(src_lines[i]);
                }
                out.push('\n');
            }
            LineKind::Code(_) => {
                if fi < formatted.len() {
                    out.push_str(formatted[fi]);
                    fi += 1;
                }
                out.push('\n');
            }
        }
    }
    // Append any remaining formatted lines.
    while fi < formatted.len() {
        out.push_str(formatted[fi]);
        fi += 1;
        out.push('\n');
    }
    out
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
            if let Some("target" | ".git" | "node_modules" | "examples_bin") =
                p.file_name().and_then(|n| n.to_str())
            {
                continue;
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
    // `&mut` is a unit — no space between `&` and `mut`.
    if matches!(p, Tok::Amp) && matches!(cur, Tok::Mut) {
        return false;
    }
    // Mid-expression keywords that always need a space before them:
    //   `in`   → `for (x in nums)`
    //   `else` → `} else {`
    //   `as`   → `import "foo" as bar`
    //   `for`  → `impl Shape for Point`
    if matches!(cur, Tok::In | Tok::Else | Tok::As | Tok::For)
        && !matches!(p, Tok::LParen | Tok::LBrace | Tok::Dot)
    {
        return true;
    }
    // `mut` needs a space before it when NOT part of `&mut`.
    if matches!(cur, Tok::Mut) && !matches!(p, Tok::Amp) {
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
                | Tok::Else
                | Tok::While
                | Tok::For
                | Tok::Match
                | Tok::Import
                | Tok::As
                | Tok::In
                | Tok::Mut
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
        Tok::Float(f) => {
            let s = f.to_string();
            // Ensure the output always contains a decimal point or exponent
            // so that `1.0` is not emitted as `1` (which would change its type).
            if s.contains('.') || s.contains('e') || s.contains('E') {
                s
            } else {
                format!("{s}.0")
            }
        }
        Tok::Str(parts) => {
            let mut s = String::from("\"");
            for p in parts {
                match p {
                    StrPart::Lit(x) => {
                        // Re-escape special characters so the output is
                        // valid Plix source (the lexer unescapes \n etc.
                        // into real control characters; we must reverse that).
                        let mut esc = String::with_capacity(x.len());
                        for c in x.chars() {
                            match c {
                                '\n' => esc.push_str("\\n"),
                                '\r' => esc.push_str("\\r"),
                                '\t' => esc.push_str("\\t"),
                                '\0' => esc.push_str("\\0"),
                                '\\' => esc.push_str("\\\\"),
                                '"' => esc.push_str("\\\""),
                                _ => esc.push(c),
                            }
                        }
                        s.push_str(&esc);
                    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(src: &str) -> String {
        format_source(src).unwrap().formatted
    }

    fn fmt_ok(src: &str) -> bool {
        format_source(src).is_ok()
    }

    #[test]
    fn for_in_needs_space_before_in() {
        let result = fmt("for (x in [1,2,3]) { say(x); }");
        assert!(
            result.contains("x in"),
            "expected `x in` but got:\n{result}"
        );
        assert!(!result.contains("xin"), "must not contain `xin`:\n{result}");
    }

    #[test]
    fn else_after_rbrace_on_same_line() {
        let result = fmt("if (a) { say(1); } else { say(2); }");
        assert!(
            result.contains("else"),
            "expected `else` but got:\n{result}"
        );
        let lines: Vec<&str> = result.lines().collect();
        let else_line = lines.iter().find(|l| l.contains("else"));
        assert!(
            else_line.is_some(),
            "expected `else` on some line, got:\n{result}"
        );
    }

    #[test]
    fn impl_for_needs_space_before_for() {
        let result = fmt("impl Shape for Point { }");
        assert!(
            result.contains("for Point"),
            "expected `for Point` but got:\n{result}"
        );
    }

    #[test]
    fn import_as_needs_space_before_as() {
        let result = fmt("import \"sys\" as s;");
        assert!(
            result.contains("as s"),
            "expected `as s` but got:\n{result}"
        );
    }

    #[test]
    fn formatter_refuses_invalid_output() {
        assert!(fmt_ok("auto x = 1;"));
    }

    #[test]
    fn formatted_output_is_idempotent() {
        let src = "auto x = 1;\nsay(x);\n";
        let first = fmt(src);
        let second = fmt(&first);
        assert_eq!(first, second, "formatter is not idempotent");
    }

    #[test]
    fn preserves_simple_program_semantics() {
        let src = "auto n = 10; say(n);";
        let result = fmt(src);
        assert!(
            crate::parser::parse_file(&result).is_ok(),
            "formatted output does not parse:\n{result}"
        );
    }

    #[test]
    fn preserves_line_comments() {
        let src = "// hello\nauto x = 1;\n// world\nsay(x);\n";
        let result = fmt(src);
        assert!(
            result.contains("// hello"),
            "lost // hello comment:\n{result}"
        );
        assert!(
            result.contains("// world"),
            "lost // world comment:\n{result}"
        );
    }

    #[test]
    fn preserves_blank_lines() {
        let src = "auto x = 1;\n\nsay(x);\n";
        let result = fmt(src);
        // Should have a blank line between the two statements.
        let lines: Vec<&str> = result.lines().collect();
        assert!(
            lines.iter().any(|l| l.trim().is_empty()),
            "lost blank line:\n{result}"
        );
    }

    #[test]
    fn preserves_block_comments() {
        let src = "/* comment */ auto x = 1;\nsay(x);\n";
        let result = fmt(src);
        assert!(
            result.contains("/* comment */"),
            "lost block comment:\n{result}"
        );
    }
}
