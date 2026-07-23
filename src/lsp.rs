//! Plix Language Server Protocol implementation.
//!
//! Provides IDE integration through the standard LSP protocol (stdin/stdout
//! JSON-RPC).  Supports:
//!   - Document synchronization (full)
//!   - Diagnostics (parse, type, ownership, lint)
//!   - Completion (keywords, builtins, user names)
//!   - Hover (type information)
//!   - Formatting (via the existing formatter)
//!   - Document symbols (functions, structs, traits, enums)
//!
//! Usage: `plix lsp`

use crate::{ast, fmt, owncheck};
use std::collections::HashMap;
use std::io::{self, BufRead, Write};

// ---------------------------------------------------------------------------
// JSON-RPC transport
// ---------------------------------------------------------------------------

fn read_message(stdin: &mut dyn BufRead) -> Option<String> {
    let mut content_length: usize = 0;
    loop {
        let mut header = String::new();
        match stdin.read_line(&mut header) {
            Ok(0) => return None,
            Ok(_) => {
                let header = header.trim();
                if header.is_empty() {
                    break;
                }
                if let Some(len_str) = header.strip_prefix("Content-Length:") {
                    content_length = len_str.trim().parse().unwrap_or(0);
                }
            }
            Err(_) => return None,
        }
    }
    if content_length == 0 {
        return None;
    }
    let mut buf = vec![0u8; content_length];
    if stdin.read_exact(&mut buf).is_err() {
        return None;
    }
    String::from_utf8(buf).ok()
}

fn write_message(stdout: &mut dyn Write, content: &str) {
    let _ = write!(
        stdout,
        "Content-Length: {}\r\n\r\n{}",
        content.len(),
        content
    );
    let _ = stdout.flush();
}

// ---------------------------------------------------------------------------
// Minimal JSON helpers
// ---------------------------------------------------------------------------

fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------------
// LSP server
// ---------------------------------------------------------------------------

type Docs = HashMap<String, String>;

pub fn run_server() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdin_lock = stdin.lock();
    let mut stdout_lock = stdout.lock();
    let mut docs: Docs = HashMap::new();

    eprintln!("plix lsp: server started");

    while let Some(msg) = read_message(&mut stdin_lock) {
        if let Some(resp) = handle_message(&msg, &mut docs) {
            write_message(&mut stdout_lock, &resp);
        }
    }

    eprintln!("plix lsp: server stopped");
}

fn handle_message(msg: &str, docs: &mut Docs) -> Option<String> {
    let method = extract_json_string_field(msg, "method");
    let id = extract_json_number_field(msg, "id");

    match method.as_deref() {
        Some("initialize") => Some(json_response(id, INIT_RESULT)),
        Some("initialized") => None,
        Some("textDocument/didOpen") => {
            let uri = extract_json_string_field(msg, "uri");
            let text = extract_json_string_field(msg, "text");
            if let (Some(uri), Some(text)) = (uri, text) {
                docs.insert(uri.clone(), text);
                let diags = compute_diagnostics(&docs[&uri]);
                Some(json_notification(
                    "textDocument/publishDiagnostics",
                    &json_publish_diagnostics(&uri, &diags),
                ))
            } else {
                None
            }
        }
        Some("textDocument/didChange") => {
            let uri = extract_json_string_field(msg, "uri");
            let text = extract_json_string_field(msg, "text");
            if let (Some(uri), Some(text)) = (uri, text) {
                docs.insert(uri.clone(), text);
                let diags = compute_diagnostics(&docs[&uri]);
                Some(json_notification(
                    "textDocument/publishDiagnostics",
                    &json_publish_diagnostics(&uri, &diags),
                ))
            } else {
                None
            }
        }
        Some("textDocument/didClose") => {
            if let Some(uri) = extract_json_string_field(msg, "uri") {
                docs.remove(&uri);
            }
            None
        }
        Some("textDocument/completion") => Some(json_response(id, COMPLETION_RESULT)),
        Some("textDocument/hover") => {
            let uri = extract_json_string_field(msg, "uri");
            let result = json_hover_result(uri.as_deref(), docs);
            Some(json_response(id, &result))
        }
        Some("textDocument/formatting") => {
            let uri = extract_json_string_field(msg, "uri");
            let result = json_formatting_result(uri.as_deref(), docs);
            Some(json_response(id, &result))
        }
        Some("textDocument/documentSymbol") => {
            let uri = extract_json_string_field(msg, "uri");
            let result = json_document_symbol_result(uri.as_deref(), docs);
            Some(json_response(id, &result))
        }
        Some("shutdown") => Some(json_response(id, "null")),
        Some("exit") => std::process::exit(0),
        _ => id.map(|i| json_error_response(i, -32601, "Method not found")),
    }
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

struct Diagnostic {
    line: u32,
    col: u32,
    severity: u32,
    code: String,
    message: String,
}

fn compute_diagnostics(source: &str) -> Vec<Diagnostic> {
    let mut diags = Vec::new();

    // 1. Lex
    let _tokens = match crate::lexer::lex(source) {
        Ok(t) => t,
        Err(e) => {
            diags.push(Diagnostic {
                line: e.line,
                col: e.col,
                severity: 1,
                code: "E0001".into(),
                message: e.msg,
            });
            return diags;
        }
    };

    // 2. Parse
    let stmts = match crate::parser::parse_file(source) {
        Ok(s) => s,
        Err(e) => {
            diags.push(Diagnostic {
                line: e.span.line,
                col: e.span.col,
                severity: 1,
                code: "E0001".into(),
                message: e.msg,
            });
            return diags;
        }
    };

    // 3. Type check
    match crate::typecheck::check_program(&stmts) {
        Ok(_) => {}
        Err(errors) => {
            for e in &errors {
                diags.push(Diagnostic {
                    line: e.span.line,
                    col: e.span.col,
                    severity: 1,
                    code: e.code.to_string(),
                    message: e.msg.clone(),
                });
            }
        }
    }

    // 4. Ownership check
    match owncheck::check_program(&stmts) {
        Ok(()) => {}
        Err(errors) => {
            for e in &errors {
                diags.push(Diagnostic {
                    line: e.span.line,
                    col: e.span.col,
                    severity: 1,
                    code: e.code.to_string(),
                    message: e.msg.clone(),
                });
            }
        }
    }

    // 5. Lint
    if let Ok(warnings) = crate::lint::lint_source(source, "<document>") {
        for w in &warnings {
            diags.push(Diagnostic {
                line: w.line,
                col: w.col,
                severity: 2,
                code: w.code.to_string(),
                message: w.msg.clone(),
            });
        }
    }

    diags
}

// ---------------------------------------------------------------------------
// JSON response builders
// ---------------------------------------------------------------------------

const INIT_RESULT: &str = r#"{"capabilities":{"textDocumentSync":1,"completionProvider":{"triggerCharacters":[".",":"]},"hoverProvider":true,"documentFormattingProvider":true,"documentSymbolProvider":true,"diagnosticProvider":{"interFileDependencies":false,"workspaceDiagnostics":false}},"serverInfo":{"name":"plix-language-server","version":"0.9.9"}}"#;

const COMPLETION_RESULT: &str = r#"{"isIncomplete":false,"items":[{"label":"func","kind":14,"detail":"keyword"},{"label":"auto","kind":14,"detail":"keyword"},{"label":"const","kind":14,"detail":"keyword"},{"label":"own","kind":14,"detail":"keyword"},{"label":"if","kind":14,"detail":"keyword"},{"label":"else","kind":14,"detail":"keyword"},{"label":"while","kind":14,"detail":"keyword"},{"label":"for","kind":14,"detail":"keyword"},{"label":"in","kind":14,"detail":"keyword"},{"label":"return","kind":14,"detail":"keyword"},{"label":"struct","kind":14,"detail":"keyword"},{"label":"impl","kind":14,"detail":"keyword"},{"label":"trait","kind":14,"detail":"keyword"},{"label":"enum","kind":14,"detail":"keyword"},{"label":"match","kind":14,"detail":"keyword"},{"label":"import","kind":14,"detail":"keyword"},{"label":"as","kind":14,"detail":"keyword"},{"label":"null","kind":14,"detail":"keyword"},{"label":"true","kind":14,"detail":"keyword"},{"label":"false","kind":14,"detail":"keyword"},{"label":"say","kind":3,"detail":"builtin"},{"label":"print","kind":3,"detail":"builtin"},{"label":"input","kind":3,"detail":"builtin"},{"label":"str","kind":3,"detail":"builtin"},{"label":"repr","kind":3,"detail":"builtin"},{"label":"int","kind":3,"detail":"builtin"},{"label":"float","kind":3,"detail":"builtin"},{"label":"bool","kind":3,"detail":"builtin"},{"label":"type_of","kind":3,"detail":"builtin"},{"label":"len","kind":3,"detail":"builtin"},{"label":"push","kind":3,"detail":"builtin"},{"label":"pop","kind":3,"detail":"builtin"},{"label":"map","kind":3,"detail":"builtin"},{"label":"filter","kind":3,"detail":"builtin"},{"label":"reduce","kind":3,"detail":"builtin"},{"label":"range","kind":3,"detail":"builtin"},{"label":"keys","kind":3,"detail":"builtin"},{"label":"values","kind":3,"detail":"builtin"},{"label":"has","kind":3,"detail":"builtin"},{"label":"get","kind":3,"detail":"builtin"},{"label":"set","kind":3,"detail":"builtin"},{"label":"sort","kind":3,"detail":"builtin"},{"label":"abs","kind":3,"detail":"builtin"},{"label":"floor","kind":3,"detail":"builtin"},{"label":"ceil","kind":3,"detail":"builtin"},{"label":"round","kind":3,"detail":"builtin"},{"label":"sqrt","kind":3,"detail":"builtin"},{"label":"pow","kind":3,"detail":"builtin"},{"label":"sin","kind":3,"detail":"builtin"},{"label":"cos","kind":3,"detail":"builtin"},{"label":"min","kind":3,"detail":"builtin"},{"label":"max","kind":3,"detail":"builtin"},{"label":"assert","kind":3,"detail":"builtin"},{"label":"assert_eq","kind":3,"detail":"builtin"},{"label":"panic","kind":3,"detail":"builtin"}]}"#;

fn json_response(id: Option<i64>, result: &str) -> String {
    match id {
        Some(id) => format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{}}}",
            id, result
        ),
        None => format!("{{\"jsonrpc\":\"2.0\",\"result\":{}}}", result),
    }
}

fn json_notification(method: &str, params: &str) -> String {
    format!(
        "{{\"jsonrpc\":\"2.0\",\"method\":\"{}\",\"params\":{}}}",
        method, params
    )
}

fn json_error_response(id: i64, code: i32, message: &str) -> String {
    format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":{},\"error\":{{\"code\":{},\"message\":{}}}}}",
        id,
        code,
        json_str(message)
    )
}

fn json_publish_diagnostics(uri: &str, diags: &[Diagnostic]) -> String {
    let items: Vec<String> = diags.iter().map(|d| format!(
        "{{\"range\":{{\"start\":{{\"line\":{},\"character\":{}}},\"end\":{{\"line\":{},\"character\":{}}}}},\"severity\":{},\"code\":{},\"message\":{}}}",
        d.line.saturating_sub(1), d.col.saturating_sub(1),
        d.line.saturating_sub(1), d.col + 10,
        d.severity, json_str(&d.code), json_str(&d.message)
    )).collect();
    format!(
        "{{\"uri\":{},\"diagnostics\":[{}]}}",
        json_str(uri),
        items.join(",")
    )
}

fn json_hover_result(uri: Option<&str>, docs: &Docs) -> String {
    if let Some(uri) = uri {
        if let Some(source) = docs.get(uri) {
            if let Ok(stmts) = crate::parser::parse_file(source) {
                let type_ok = crate::typecheck::check_program(&stmts).is_ok();
                let own_ok = owncheck::check_program(&stmts).is_ok();
                let info = format!(
                    "Plix program\nTypes: {}\nOwnership: {}",
                    if type_ok { "✓" } else { "✗" },
                    if own_ok { "✓" } else { "✗" }
                );
                return format!(
                    "{{\"contents\":{{\"kind\":\"plaintext\",\"value\":{}}}}}",
                    json_str(&info)
                );
            }
        }
    }
    "null".to_string()
}

fn json_formatting_result(uri: Option<&str>, docs: &Docs) -> String {
    if let Some(uri) = uri {
        if let Some(source) = docs.get(uri) {
            match fmt::format_source(source) {
                Ok(result) if result.changed => {
                    return format!(
                        "[{{\"range\":{{\"start\":{{\"line\":0,\"character\":0}},\"end\":{{\"line\":999999,\"character\":0}}}},\"newText\":{}}}]",
                        json_str(&result.formatted)
                    );
                }
                _ => return "[]".to_string(),
            }
        }
    }
    "[]".to_string()
}

fn json_document_symbol_result(uri: Option<&str>, docs: &Docs) -> String {
    if let Some(uri) = uri {
        if let Some(source) = docs.get(uri) {
            if let Ok(stmts) = crate::parser::parse_file(source) {
                let symbols: Vec<String> = stmts.iter().filter_map(|s| match &s.node {
                    ast::StmtKind::Func(f) => Some(format!(
                        "{{\"name\":{},\"kind\":12,\"range\":{{\"start\":{{\"line\":{},\"character\":0}},\"end\":{{\"line\":{},\"character\":0}}}}}}",
                        json_str(&f.name), s.span.line - 1, s.span.line - 1)),
                    ast::StmtKind::Struct { name, .. } => Some(format!(
                        "{{\"name\":{},\"kind\":23,\"range\":{{\"start\":{{\"line\":{},\"character\":0}},\"end\":{{\"line\":{},\"character\":0}}}}}}",
                        json_str(name), s.span.line - 1, s.span.line - 1)),
                    ast::StmtKind::Enum { name, .. } => Some(format!(
                        "{{\"name\":{},\"kind\":10,\"range\":{{\"start\":{{\"line\":{},\"character\":0}},\"end\":{{\"line\":{},\"character\":0}}}}}}",
                        json_str(name), s.span.line - 1, s.span.line - 1)),
                    ast::StmtKind::Trait { name, .. } => Some(format!(
                        "{{\"name\":{},\"kind\":11,\"range\":{{\"start\":{{\"line\":{},\"character\":0}},\"end\":{{\"line\":{},\"character\":0}}}}}}",
                        json_str(name), s.span.line - 1, s.span.line - 1)),
                    _ => None,
                }).collect();
                return format!("[{}]", symbols.join(","));
            }
        }
    }
    "[]".to_string()
}

fn extract_json_string_field(json: &str, field: &str) -> Option<String> {
    let needle = format!("\"{}\":\"", field);
    let start = json.find(&needle)?;
    let val_start = start + needle.len();
    let val_end = json[val_start..].find('"')?;
    Some(json[val_start..val_start + val_end].to_string())
}

fn extract_json_number_field(json: &str, field: &str) -> Option<i64> {
    let needle = format!("\"{}\":", field);
    let start = json.find(&needle)?;
    let val_start = start + needle.len();
    let rest = json[val_start..].trim_start();
    let end = rest.find(|c: char| !c.is_ascii_digit() && c != '-')?;
    rest[..end].parse().ok()
}
