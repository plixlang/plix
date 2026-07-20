//! plix — the Plix language toolchain.
//!
//!   plix run    file.px            interpret (great errors, instant start)
//!   plix build  file.px -o app     compile to a standalone native executable
//!   plix exec   file.px            compile to a temp binary and run it
//!   plix check  file.px            syntax + ownership checks only
//!   plix repl                      interactive shell
//!   plix --version

mod ast;
mod owncheck;
// codegen modules (native compiler)
mod codegen;
mod interp;
mod lexer;
mod parser;
mod resolve;
mod token;
mod typecheck;

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

const VERSION: &str = "0.3.0";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        usage();
        return ExitCode::from(2);
    }
    match args[1].as_str() {
        "run" => cmd_run(&args[2..], false),
        "build" => cmd_build(&args[2..]),
        "exec" => cmd_run(&args[2..], true),
        "check" => cmd_check(&args[2..]),
        "test" => cmd_test(&args[2..]),
        "repl" => cmd_repl(),
        "--version" | "-V" | "version" => {
            println!("plix {}", VERSION);
            ExitCode::SUCCESS
        }
        "--help" | "-h" | "help" => {
            usage();
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("unknown command \"{}\"", other);
            usage();
            ExitCode::from(2)
        }
    }
}

fn usage() {
    eprintln!(
        "plix {} — the Plix language

USAGE:
  plix run    <file.px>           interpret the program (fast startup)
  plix build  <file.px> -o <out>  compile to a native executable
  plix exec   <file.px>           compile to memory and run natively
  plix check  <file.px>           parse + ownership checks (no execution)
  plix test   [paths...]          run *_test.px suites (default: ./tests)
  plix repl                       interactive shell
  plix --version

ENV:
  PLIX_PYTHON_LIB   path to libpython3.x.so (overrides autodetect)",
        VERSION
    );
}

fn read_file_arg(args: &[String]) -> Result<(PathBuf, String), String> {
    let file = args
        .first()
        .ok_or_else(|| "missing input file".to_string())?;
    let path = PathBuf::from(file);
    let src = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
    Ok((path, src))
}

/// recursive `*_test.px` discovery (no external walk crate needed)
fn collect_test_files(dir: &PathBuf, out: &mut Vec<PathBuf>) {
    let rd = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return,
    };
    for ent in rd.flatten() {
        let p = ent.path();
        let is_dir = p.is_dir();
        if is_dir {
            // skip tool/build scratch dirs
            match p.file_name().and_then(|n| n.to_str()) {
                Some("target" | ".git" | "node_modules") => continue,
                _ => {}
            }
            collect_test_files(&p, out);
        } else if p.extension().map(|e| e == "px").unwrap_or(false)
            && p.file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.ends_with("_test"))
                .unwrap_or(false)
        {
            out.push(p);
        }
    }
}

fn cmd_test(args: &[String]) -> ExitCode {
    let roots: Vec<String> = if args.is_empty() {
        vec![if PathBuf::from("tests").is_dir() {
            "tests".to_string()
        } else {
            ".".to_string()
        }]
    } else {
        args.to_vec()
    };
    let mut files: Vec<PathBuf> = Vec::new();
    for r in &roots {
        let p = PathBuf::from(r);
        if p.is_file() {
            files.push(p);
        } else if p.is_dir() {
            collect_test_files(&p, &mut files);
        } else {
            eprintln!("error: no such file or directory: {}", r);
            return ExitCode::FAILURE;
        }
    }
    files.sort();
    files.dedup();
    if files.is_empty() {
        println!("no *_test.px files found");
        return ExitCode::SUCCESS;
    }

    let mut total = 0usize;
    let mut failed = 0usize;
    for f in &files {
        let name = f.display().to_string();
        let src = match std::fs::read_to_string(f) {
            Ok(s) => s,
            Err(e) => {
                println!("✗ {} (cannot read: {})", name, e);
                failed += 1;
                continue;
            }
        };
        let base = f
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        let mut it = interp::Interpreter::new(base);
        match interp::run_test_file(&src, &name, &mut it) {
            Err(e) => {
                println!("✗ {}", name);
                println!("    {}", e.lines().next().unwrap_or(&e));
                total += 1;
                failed += 1;
            }
            Ok(outcomes) => {
                let fails: Vec<&interp::TestOutcome> =
                    outcomes.iter().filter(|o| o.result.is_err()).collect();
                total += outcomes.len().max(0);
                failed += fails.len();
                if fails.is_empty() {
                    println!("✓ {} ({} tests)", name, outcomes.len());
                } else {
                    println!("✗ {} ({}/{} failed)", name, fails.len(), outcomes.len());
                    for o in fails {
                        if let Err(m) = &o.result {
                            println!("    ✗ {} — {}", o.name, m);
                        }
                    }
                }
            }
        }
    }
    println!();
    println!(
        "{} test(s): {} passed, {} failed",
        total,
        total.saturating_sub(failed),
        failed
    );
    if failed > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn cmd_run(args: &[String], native: bool) -> ExitCode {
    let (path, src) = match read_file_arg(args) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("error: {}", e);
            return ExitCode::FAILURE;
        }
    };
    let name = path.display().to_string();
    let base = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    // sys.args(): [program, trailing args...]
    let mut script_args = vec![path.display().to_string()];
    if args.len() > 1 {
        script_args.extend(args[1..].iter().cloned());
    }
    plixrt::builtins::set_program_args(script_args);
    if native {
        return match codegen::compile_and_exec(&src, &name, &args[1..]) {
            Ok(code) => ExitCode::from(code),
            Err(e) => {
                eprintln!("{}", e);
                ExitCode::FAILURE
            }
        };
    }
    let mut it = interp::Interpreter::new(base);
    match interp::run_program(&src, &name, PathBuf::from("."), &mut it) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}", e);
            ExitCode::FAILURE
        }
    }
}

fn cmd_build(args: &[String]) -> ExitCode {
    let (path, src) = match read_file_arg(args) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("error: {}", e);
            return ExitCode::FAILURE;
        }
    };
    let mut out = "a.out".to_string();
    let mut i = 1;
    while i < args.len() {
        if (args[i] == "-o" || args[i] == "--output") && i + 1 < args.len() {
            out = args[i + 1].clone();
            i += 2;
        } else {
            i += 1;
        }
    }
    let name = path.display().to_string();
    match codegen::compile_to_executable(&src, &name, &out) {
        Ok(()) => {
            println!("✓ built native executable: {}", out);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{}", e);
            ExitCode::FAILURE
        }
    }
}

fn cmd_check(args: &[String]) -> ExitCode {
    let (path, src) = match read_file_arg(args) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("error: {}", e);
            return ExitCode::FAILURE;
        }
    };
    let name = path.display().to_string();
    let stmts = match parser::parse_file(&src) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "{}:{}:{}: syntax error: {}",
                name, e.span.line, e.span.col, e.msg
            );
            return ExitCode::FAILURE;
        }
    };
    let mut failed = false;
    if let Err(errs) = typecheck::check_program(&stmts) {
        eprint!("{}", owncheck::format_errors(&errs, &src, &name));
        failed = true;
    }
    if let Err(errs) = owncheck::check_program(&stmts) {
        eprint!("{}", owncheck::format_errors(&errs, &src, &name));
        failed = true;
    }
    if failed {
        ExitCode::FAILURE
    } else {
        println!("✓ {}: no errors", name);
        ExitCode::SUCCESS
    }
}

// ---------------------------------------------------------------------------
// REPL
// ---------------------------------------------------------------------------

fn cmd_repl() -> ExitCode {
    println!("plix {} repl — :quit to exit, :help for commands", VERSION);
    let mut it = interp::Interpreter::new(PathBuf::from("."));
    let stdin = std::io::stdin();
    let mut n = 0u32;
    // the checker needs to see earlier declarations (structs, functions),
    // so snippets are checked against the accumulated history (execution
    // still runs only the new snippet)
    let mut history = String::new();
    loop {
        n += 1;
        print!("plix:{}> ", n);
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        match stdin.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        let src = line.trim().to_string();
        if src.is_empty() {
            continue;
        }
        match src.as_str() {
            ":q" | ":quit" | ":exit" => break,
            ":h" | ":help" => {
                println!("commands: :quit  :help  :globals  — anything else is Plix code");
                continue;
            }
            ":globals" => {
                continue;
            }
            _ => {}
        }
        // collect continuation lines if brackets are unbalanced
        let mut full = line.clone();
        while unbalanced(&full) {
            print!("  ...> ");
            let _ = std::io::stdout().flush();
            let mut more = String::new();
            match stdin.read_line(&mut more) {
                Ok(0) => break,
                Ok(_) => full.push_str(&more),
                Err(_) => break,
            }
        }
        // REPL convenience: a missing trailing ';' is fine there
        if !full.trim_end().ends_with(';') && !full.trim_end().ends_with('}') {
            full.push(';');
        }
        let checked = if history.is_empty() {
            full.clone()
        } else {
            format!("{}\n{}", history, full)
        };
        match parser::parse_file(&checked) {
            Ok(checked_stmts) => {
                if let Err(errs) = typecheck::check_program(&checked_stmts) {
                    eprint!("{}", owncheck::format_errors(&errs, &checked, "<repl>"));
                    continue;
                }
                if let Err(errs) = owncheck::check_program(&checked_stmts) {
                    eprint!("{}", owncheck::format_errors(&errs, &checked, "<repl>"));
                    continue;
                }
                let stmts = match parser::parse_file(&full) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("{}:{}: syntax error: {}", e.span.line, e.span.col, e.msg);
                        continue;
                    }
                };
                // install fresh type info (whole session) for impl lookups
                if let Ok(info) = typecheck::check_program(&checked_stmts) {
                    it.tinfo = std::rc::Rc::new(info);
                }
                let show_value = stmts.len() == 1
                    && matches!(stmts[0].node, ast::StmtKind::ExprStmt(_));
                let prog = ast::Program {
                    stmts,
                    source_name: "<repl>".into(),
                };
                if show_value {
                    // evaluate expression manually to display the result
                    let e = match &prog.stmts[0].node {
                        ast::StmtKind::ExprStmt(e) => e.clone(),
                        _ => unreachable!(),
                    };
                    match it.eval_pub(&e) {
                        Ok(v) => {
                            if !plixrt::heap::is_null(v) {
                                println!("{}", plixrt::value::to_repr(v));
                            }
                        }
                        Err(e) => eprintln!("{}:{}: RuntimeError: {}", e.line, e.col, e.msg),
                    }
                    continue;
                }
                match it.run(&prog) {
                    Err(e) => eprintln!("{}:{}: RuntimeError: {}", e.line, e.col, e.msg),
                    Ok(()) => {
                        history.push_str(&full);
                        history.push('\n');
                    }
                }
            }
            Err(e) => eprintln!("{}:{}: syntax error: {}", e.span.line, e.span.col, e.msg),
        }
    }
    println!("bye");
    ExitCode::SUCCESS
}

fn unbalanced(s: &str) -> bool {
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    for c in s.chars() {
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' => depth -= 1,
            _ => {}
        }
    }
    depth > 0 || in_str
}
