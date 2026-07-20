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
mod fmt;
mod interp;
mod lexer;
mod lint;
mod manifest;
mod parser;
mod resolve;
mod token;
mod typecheck;

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

const VERSION: &str = "0.5.0";

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
        "fmt" => cmd_fmt(&args[2..]),
        "lint" => cmd_lint(&args[2..]),
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
  plix run    [file.px] [args...] interpret the program (uses plix.toml if omitted)
  plix build  [file.px] -o <out>  compile to a native executable
  plix exec   [file.px] [args...] compile and run natively
  plix check  <file.px>           parse + type + ownership checks
  plix test   [opts] [paths...]   run *_test.px suites (--filter, --fail-fast, --json)
  plix fmt    [--check] [paths...] format .px files
  plix lint   [paths...]          lint .px files
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
        .filter(|s| !s.starts_with('-'))
        .cloned()
        .or_else(manifest_entry_file)
        .ok_or_else(|| {
            "missing input file (pass file.px or add [build].entry to plix.toml)".to_string()
        })?;
    let path = PathBuf::from(file);
    let src = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
    Ok((path, src))
}

fn manifest_entry_file() -> Option<String> {
    let cwd = std::env::current_dir().ok()?;
    let mp = manifest::Manifest::find_from(&cwd)?;
    manifest::Manifest::load_from(&mp).ok()?.build_entry
}

fn manifest_build_out() -> Option<String> {
    let cwd = std::env::current_dir().ok()?;
    let mp = manifest::Manifest::find_from(&cwd)?;
    manifest::Manifest::load_from(&mp).ok()?.build_out
}

fn manifest_test_paths() -> Option<Vec<String>> {
    let cwd = std::env::current_dir().ok()?;
    let mp = manifest::Manifest::find_from(&cwd)?;
    let paths = manifest::Manifest::load_from(&mp).ok()?.test_paths;
    if paths.is_empty() { None } else { Some(paths) }
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

#[derive(Default)]
struct TestOpts {
    filter: Option<String>,
    fail_fast: bool,
    json: bool,
    paths: Vec<String>,
}

fn parse_test_opts(args: &[String]) -> Result<TestOpts, String> {
    let mut o = TestOpts::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--filter" => {
                i += 1;
                o.filter = Some(args.get(i).ok_or("--filter needs a value")?.clone());
            }
            "--fail-fast" => o.fail_fast = true,
            "--json" => o.json = true,
            "-v" | "--verbose" => {}
            x if x.starts_with('-') => return Err(format!("unknown test option {}", x)),
            x => o.paths.push(x.to_string()),
        }
        i += 1;
    }
    Ok(o)
}

fn cmd_test(args: &[String]) -> ExitCode {
    let opts = match parse_test_opts(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("error: {}", e);
            return ExitCode::from(2);
        }
    };
    let roots: Vec<String> = if opts.paths.is_empty() {
        manifest_test_paths().unwrap_or_else(|| {
            vec![if PathBuf::from("tests").is_dir() {
                "tests".to_string()
            } else {
                ".".to_string()
            }]
        })
    } else {
        opts.paths.clone()
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
        if opts.json {
            println!("{{\"total\":0,\"passed\":0,\"failed\":0,\"files\":[]}}");
        } else {
            println!("no *_test.px files found");
        }
        return ExitCode::SUCCESS;
    }

    let mut total = 0usize;
    let mut failed = 0usize;
    let mut json_files: Vec<String> = Vec::new();
    'files: for f in &files {
        let name = f.display().to_string();
        let src = match std::fs::read_to_string(f) {
            Ok(s) => s,
            Err(e) => {
                failed += 1;
                if !opts.json {
                    println!("✗ {} (cannot read: {})", name, e);
                }
                if opts.fail_fast {
                    break;
                }
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
                total += 1;
                failed += 1;
                if opts.json {
                    json_files.push(format!(
                        "{{\"file\":\"{}\",\"tests\":[{{\"name\":\"<file>\",\"ok\":false,\"message\":\"{}\"}}]}}",
                        json_escape(&name),
                        json_escape(&e)
                    ));
                } else {
                    println!("✗ {}", name);
                    println!("    {}", e.lines().next().unwrap_or(&e));
                }
            }
            Ok(outcomes) => {
                let selected: Vec<&interp::TestOutcome> = outcomes
                    .iter()
                    .filter(|o| {
                        opts.filter
                            .as_ref()
                            .map(|f| o.name.contains(f))
                            .unwrap_or(true)
                    })
                    .collect();
                let fails: Vec<&&interp::TestOutcome> =
                    selected.iter().filter(|o| o.result.is_err()).collect();
                total += selected.len();
                failed += fails.len();
                if opts.json {
                    let tests: Vec<String> = selected
                        .iter()
                        .map(|o| match &o.result {
                            Ok(()) => format!(
                                "{{\"name\":\"{}\",\"line\":{},\"ok\":true}}",
                                json_escape(&o.name),
                                o.line
                            ),
                            Err(m) => format!(
                                "{{\"name\":\"{}\",\"line\":{},\"ok\":false,\"message\":\"{}\"}}",
                                json_escape(&o.name),
                                o.line,
                                json_escape(m)
                            ),
                        })
                        .collect();
                    json_files.push(format!(
                        "{{\"file\":\"{}\",\"tests\":[{}]}}",
                        json_escape(&name),
                        tests.join(",")
                    ));
                } else if fails.is_empty() {
                    println!("✓ {} ({} tests)", name, selected.len());
                } else {
                    println!("✗ {} ({}/{} failed)", name, fails.len(), selected.len());
                    for o in &fails {
                        if let Err(m) = &o.result {
                            println!("    ✗ {} — {}", o.name, m);
                        }
                    }
                }
                if opts.fail_fast && !fails.is_empty() {
                    break 'files;
                }
            }
        }
    }
    if opts.json {
        println!(
            "{{\"total\":{},\"passed\":{},\"failed\":{},\"files\":[{}]}}",
            total,
            total.saturating_sub(failed),
            failed,
            json_files.join(",")
        );
    } else {
        println!();
        println!(
            "{} test(s): {} passed, {} failed",
            total,
            total.saturating_sub(failed),
            failed
        );
    }
    if failed > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn json_escape(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            '\\' => "\\\\".chars().collect::<Vec<_>>(),
            '"' => "\\\"".chars().collect(),
            '\n' => "\\n".chars().collect(),
            '\r' => "\\r".chars().collect(),
            '\t' => "\\t".chars().collect(),
            x => vec![x],
        })
        .collect()
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
    let mut out = manifest_build_out().unwrap_or_else(|| "a.out".to_string());
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

fn cmd_fmt(args: &[String]) -> ExitCode {
    let mut check = false;
    let mut paths = Vec::new();
    for a in args {
        if a == "--check" {
            check = true;
        } else {
            paths.push(PathBuf::from(a));
        }
    }
    if paths.is_empty() {
        paths.push(PathBuf::from("."));
    }
    let mut files = Vec::new();
    for p in &paths {
        fmt::collect_px_files(p, &mut files);
    }
    files.sort();
    files.dedup();
    let mut failed = false;
    for f in files {
        let src = match std::fs::read_to_string(&f) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("fmt: cannot read {}: {}", f.display(), e);
                failed = true;
                continue;
            }
        };
        match fmt::format_source(&src) {
            Ok(r) => {
                if r.changed {
                    if check {
                        println!("would reformat {}", f.display());
                        failed = true;
                    } else if let Err(e) = std::fs::write(&f, r.formatted) {
                        eprintln!("fmt: cannot write {}: {}", f.display(), e);
                        failed = true;
                    } else {
                        println!("formatted {}", f.display());
                    }
                }
            }
            Err(e) => {
                eprintln!("fmt: {}: {}", f.display(), e);
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn cmd_lint(args: &[String]) -> ExitCode {
    let mut paths: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();
    if paths.is_empty() {
        paths.push(PathBuf::from("."));
    }
    let mut files = Vec::new();
    for p in &paths {
        fmt::collect_px_files(p, &mut files);
    }
    files.sort();
    files.dedup();
    let mut total = 0usize;
    let mut failed = false;
    for f in files {
        let name = f.display().to_string();
        let src = match std::fs::read_to_string(&f) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("lint: cannot read {}: {}", name, e);
                failed = true;
                continue;
            }
        };
        match lint::lint_source(&src, &name) {
            Ok(ws) => {
                for w in ws {
                    println!(
                        "warning[{}]: {}\n  --> {}:{}:{}",
                        w.code, w.msg, name, w.line, w.col
                    );
                    total += 1;
                }
            }
            Err(e) => {
                eprint!("{}", e);
                failed = true;
            }
        }
    }
    if total == 0 && !failed {
        println!("✓ no lint warnings");
    } else if total > 0 {
        println!("{} warning(s)", total);
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
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
                let show_value =
                    stmts.len() == 1 && matches!(stmts[0].node, ast::StmtKind::ExprStmt(_));
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
