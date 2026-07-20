//! Native code generation: Plix -> Cranelift IR -> object file -> linked
//! standalone executable (system cc + embedded libplixrt.a).
//!
//! Model:
//!   - every Plix function compiles to one native function with ABI
//!       fn(cells: *const V, args: *const V, nargs: i64) -> V
//!   - `V` is the 64-bit tagged value of the runtime; ints stay unboxed, so
//!     int arithmetic / comparisons are inlined, everything else calls the
//!     shared runtime (the same semantics the interpreter uses);
//!   - variables live in registers/stack slots; variables captured by
//!     closures live in heap Cells (resolve.rs computes them). All locals
//!     are pre-declared at the entry block (initialized to null) so every
//!     variable is always defined on every path: declaration then behaves
//!     like a store. Reading a local before its declaration line therefore
//!     yields null instead of being an SSA error;
//!   - statement temporaries live in the frame arena and are freed by
//!     arena rewinds (per expression statement + per loop iteration);
//!   - C `main` = plix_rt_init + install builtins + run plix_main + report
//!     runtime errors.
//!
//! Refcount discipline (matches rt/src/heap.rs):
//!   - every runtime op result is arena-owned ("temp");
//!   - storing a temp into a variable retains it (+1); the variable's hold
//!     is released once in the epilogue (locals list);
//!   - `return v`: retain once, hand to plix_frame_pop which adopts the
//!     value into the caller's arena (no further release).

use crate::ast::*;
use crate::parser;
use crate::resolve::{self, MAIN_RES_ID, Resolution};
use cranelift_codegen::Context;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::types;
use cranelift_codegen::ir::{
    AbiParam, Block, InstBuilder, MemFlags, Signature, StackSlotData, StackSlotKind, Value as CVal,
};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

const I64: cranelift_codegen::ir::Type = types::I64;
const TNULL: i64 = 0;
const TTRUE: i64 = 2;
const TFALSE: i64 = 6;

/// boxed-int domain (63-bit tagged) mirrored from rt/src/heap.rs
const INT_MIN: i64 = -(1i64 << 62);
const INT_MAX: i64 = (1i64 << 62) - 1;
/// tagged extremes: tag(INT_MIN) and tag(INT_MAX), both fit in i64
const TAGGED_MIN: i64 = i64::MIN + 1;
const TAGGED_MAX: i64 = i64::MAX;

#[inline]
fn tconst_int(i: i64) -> i64 {
    (i << 1) | 1
}

type CResult<T> = Result<T, String>;

// ---------------------------------------------------------------------------
// pipeline
// ---------------------------------------------------------------------------

pub fn compile_to_executable(src: &str, name: &str, out: &str) -> Result<(), String> {
    let obj = build_object(src, name)?;
    link_executable(&obj, out)
}

pub fn compile_and_exec(src: &str, name: &str, extra_args: &[String]) -> Result<u8, String> {
    let dir = std::env::temp_dir().join(format!("plix_exec_{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let exe = dir.join("a.out");
    compile_to_executable(src, name, &exe.to_string_lossy())?;
    let st = std::process::Command::new(&exe)
        .args(extra_args)
        .status()
        .map_err(|e| format!("cannot run {}: {}", exe.display(), e))?;
    Ok(st.code().unwrap_or(1) as u8)
}

#[derive(Clone)]
struct ModMeta {
    alias: String,
    init_fid: FuncId,
    exports: Vec<(String, usize)>,
}

struct ModUnit {
    alias: String,
    stmts: Vec<Stmt>,
    res: Option<Resolution>,
    tinfo: Option<crate::typecheck::TypeInfo>,
    init_fid: Option<FuncId>,
    flag_idx: usize,
}

fn build_object(src: &str, name: &str) -> Result<Vec<u8>, String> {
    let stmts = parser::parse_file(src).map_err(|e| {
        format!(
            "{}:{}:{}: syntax error: {}",
            name, e.span.line, e.span.col, e.msg
        )
    })?;
    let main_tinfo = crate::typecheck::check_program(&stmts)
        .map_err(|errs| crate::owncheck::format_errors(&errs, src, name))?;
    crate::owncheck::check_program(&stmts)
        .map_err(|errs| crate::owncheck::format_errors(&errs, src, name))?;

    let base_dir = PathBuf::from(name)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let mut mods: Vec<ModUnit> = Vec::new();
    for s in &stmts {
        if let StmtKind::Import {
            module,
            alias,
            python,
        } = &s.node
        {
            if *python || !module.ends_with(".px") {
                continue;
            }
            let path = base_dir.join(module);
            let msrc = std::fs::read_to_string(&path).map_err(|e| {
                format!(
                    "{}:{}:{}: cannot read module {}: {}",
                    name, s.span.line, s.span.col, module, e
                )
            })?;
            let mstmts = parser::parse_file(&msrc).map_err(|e| {
                format!(
                    "{}:{}:{}: syntax error: {}",
                    path.display(),
                    e.span.line,
                    e.span.col,
                    e.msg
                )
            })?;
            let mtinfo = crate::typecheck::check_program(&mstmts).map_err(|errs| {
                crate::owncheck::format_errors(&errs, &msrc, &path.display().to_string())
            })?;
            crate::owncheck::check_program(&mstmts).map_err(|errs| {
                crate::owncheck::format_errors(&errs, &msrc, &path.display().to_string())
            })?;
            mods.push(ModUnit {
                alias: alias.clone(),
                stmts: mstmts,
                res: None,
                tinfo: Some(mtinfo),
                init_fid: None,
                flag_idx: 0,
            });
        }
    }

    let builtin_names: Vec<String> = plixrt::builtins::global_names()
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut offset = builtin_names.len();
    let main_res = resolve::resolve_program_with_base(&stmts, &builtin_names, offset)
        .map_err(render_res_errors)?;
    offset += main_res.user_globals.len();
    for m in &mut mods {
        let r = resolve::resolve_program_with_base(&m.stmts, &builtin_names, offset)
            .map_err(render_res_errors)?;
        offset += r.user_globals.len();
        m.res = Some(r);
    }
    for (i, m) in mods.iter_mut().enumerate() {
        m.flag_idx = offset + i;
    }
    let total_globals = offset + mods.len();

    let mut assigned_globals: HashSet<String> = HashSet::new();
    collect_assigned_idents(&stmts, &mut assigned_globals);

    let mut cfg = settings::builder();
    let _ = cfg.set("opt_level", "speed");
    let _ = cfg.set("is_pic", "true");
    let flags = settings::Flags::new(cfg);
    let isa = cranelift_native::builder()
        .map_err(|e| format!("cannot create host ISA: {}", e))?
        .finish(flags)
        .map_err(|e| format!("cannot finish ISA: {}", e))?;
    let obj = ObjectBuilder::new(
        isa,
        "plix_program",
        cranelift_module::default_libcall_names(),
    )
    .map_err(|e| format!("object builder: {}", e))?;

    let mut c = Compiler {
        ctx: Context::new(),
        fn_ctx: FunctionBuilderContext::new(),
        shared: Shared {
            module: ObjectModule::new(obj),
            rt: HashMap::new(),
            strings: HashMap::new(),
            data_counter: 0,
        },
        fns: HashMap::new(),
        fn_name_of: HashMap::new(),
        sig: make_sig(&[I64, I64, I64], &[I64]),
        assigned_globals,
    };

    // declare all functions up-front (recursion safe)
    let mut all: Vec<(Rc<FuncDef>, usize)> = Vec::new();
    {
        let mut ds: Vec<Rc<FuncDef>> = Vec::new();
        collect_all_fn_defs(&stmts, &mut ds);
        for d in ds {
            all.push((d, 0usize));
        }
        for (i, m) in mods.iter().enumerate() {
            let mut ds2: Vec<Rc<FuncDef>> = Vec::new();
            collect_all_fn_defs(&m.stmts, &mut ds2);
            for d in ds2 {
                all.push((d, i + 1));
            }
        }
    }
    for (f, unit) in &all {
        let id = FuncDef::id(f);
        if c.fns.contains_key(&id) {
            continue;
        }
        let fname = format!("plix_fn_{}_{}", sanitize(&f.name), id);
        let fid = c
            .shared
            .module
            .declare_function(&fname, Linkage::Local, &c.sig.clone())
            .map_err(|e| e.to_string())?;
        c.fns.insert(id, fid);
        c.fn_name_of.insert(id, (f.name.clone(), *unit));
    }

    let plix_main_id = c
        .shared
        .module
        .declare_function("plix_main", Linkage::Local, &c.sig.clone())
        .map_err(|e| e.to_string())?;
    for (i, m) in mods.iter_mut().enumerate() {
        let fid = c
            .shared
            .module
            .declare_function(
                &format!("plix_mod_init_{}", i),
                Linkage::Local,
                &c.sig.clone(),
            )
            .map_err(|e| e.to_string())?;
        m.init_fid = Some(fid);
    }

    // module init bodies
    for m in &mods {
        let res = m.res.as_ref().unwrap();
        let tinfo = m.tinfo.as_ref().unwrap();
        let fid = m.init_fid.unwrap();
        c.compile_unit_pseudo_fn(
            fid,
            &m.stmts,
            res,
            tinfo,
            m.flag_idx,
            &[],
            &format!("module {}", m.alias),
        )?;
    }

    // user functions
    for (def, unit) in &all {
        let (res, tinfo) = if *unit == 0 {
            (&main_res, &main_tinfo)
        } else {
            (
                mods[*unit - 1].res.as_ref().unwrap(),
                mods[*unit - 1].tinfo.as_ref().unwrap(),
            )
        };
        c.compile_function(FuncDef::id(def), def, res, tinfo)?;
    }

    // main
    let mod_meta: Vec<ModMeta> = mods
        .iter()
        .map(|m| {
            let r = m.res.as_ref().unwrap();
            ModMeta {
                alias: m.alias.clone(),
                init_fid: m.init_fid.unwrap(),
                exports: r
                    .user_globals
                    .iter()
                    .map(|n| (n.clone(), *r.globals.get(n).unwrap()))
                    .collect(),
            }
        })
        .collect();
    c.compile_unit_pseudo_fn(
        plix_main_id,
        &stmts,
        &main_res,
        &main_tinfo,
        usize::MAX,
        &mod_meta,
        "main",
    )?;
    c.compile_c_main(plix_main_id, total_globals as i64)?;

    let product = c.shared.module.finish();
    product.emit().map_err(|e| format!("emit object: {}", e))
}

fn render_res_errors(errs: Vec<resolve::ResErr>) -> String {
    let mut out = String::new();
    for e in errs {
        out.push_str(&format!("error: {} (at {}:{})\n", e.msg, e.line, e.col));
    }
    out
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

fn make_sig(
    params: &[cranelift_codegen::ir::Type],
    rets: &[cranelift_codegen::ir::Type],
) -> Signature {
    let mut sig = Signature::new(cranelift_codegen::isa::CallConv::SystemV);
    for p in params {
        sig.params.push(AbiParam::new(*p));
    }
    for r in rets {
        sig.returns.push(AbiParam::new(*r));
    }
    sig
}

// ---------------------------------------------------------------------------
// shared compiler state
// ---------------------------------------------------------------------------

struct Shared {
    module: ObjectModule,
    rt: HashMap<&'static str, FuncId>,
    strings: HashMap<String, DataId>,
    data_counter: usize,
}

impl Shared {
    fn rt_id(&mut self, name: &'static str, nparams: usize) -> CResult<FuncId> {
        if let Some(&id) = self.rt.get(name) {
            return Ok(id);
        }
        let sig = make_sig(&vec![I64; nparams], &[I64]);
        let id = self
            .module
            .declare_function(name, Linkage::Import, &sig)
            .map_err(|e| format!("declare {}: {}", name, e))?;
        self.rt.insert(name, id);
        Ok(id)
    }

    /// runtime function with a fully custom signature (e.g. f64 ABI for the
    /// unboxed specialization paths)
    fn rt_id_typed(
        &mut self,
        name: &'static str,
        params: &[cranelift_codegen::ir::Type],
        rets: &[cranelift_codegen::ir::Type],
    ) -> CResult<FuncId> {
        if let Some(&id) = self.rt.get(name) {
            return Ok(id);
        }
        let sig = make_sig(params, rets);
        let id = self
            .module
            .declare_function(name, Linkage::Import, &sig)
            .map_err(|e| format!("declare {}: {}", name, e))?;
        self.rt.insert(name, id);
        Ok(id)
    }

    fn str_data(&mut self, s: &str) -> CResult<DataId> {
        if let Some(&d) = self.strings.get(s) {
            return Ok(d);
        }
        self.data_counter += 1;
        let dname = format!("plix_str_{}", self.data_counter);
        let did = self
            .module
            .declare_data(&dname, Linkage::Local, false, false)
            .map_err(|e| e.to_string())?;
        let mut dd = DataDescription::new();
        dd.define(s.as_bytes().to_vec().into_boxed_slice());
        self.module
            .define_data(did, &dd)
            .map_err(|e| e.to_string())?;
        self.strings.insert(s.to_string(), did);
        Ok(did)
    }
}

struct Compiler {
    ctx: Context,
    fn_ctx: FunctionBuilderContext,
    shared: Shared,
    fns: HashMap<usize, FuncId>,
    fn_name_of: HashMap<usize, (String, usize)>,
    sig: Signature,
    assigned_globals: HashSet<String>,
}

// ---------------------------------------------------------------------------
// per-function emitter
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Loc {
    /// variable holding a plain value
    Local(Variable),
    /// variable holding a pointer to a heap Cell (captured by a closure)
    Cell(Variable),
    /// runtime global slot
    Global(usize),
    /// captured cell of an enclosing function (index into cells array)
    Free(usize),
    /// specialization: raw unboxed i64 (provable-typed int local)
    RawInt(Variable),
    /// specialization: raw unboxed f64 (provable-typed float local)
    RawFloat(Variable),
    /// specialization: raw 0/1 i64 (provable-typed bool local)
    RawBool(Variable),
}

#[derive(Clone, Copy, PartialEq)]
enum STy {
    /// statically provable int (raw i64 emission available)
    Int,
    /// statically provable float (raw f64 emission available)
    Float,
    /// statically provable bool (raw 0/1 emission available)
    Bool,
    /// anything else — boxed V semantics
    Box,
}

/// a value in a raw (unboxed) representation
#[derive(Clone, Copy)]
enum RawVal {
    I(CVal),
    F(CVal),
    B(CVal),
}

#[derive(Clone, Copy, PartialEq)]
enum Want {
    Int,
    Float,
    Bool,
}

const F64_TY: cranelift_codegen::ir::Type = types::F64;

fn guard_flags_of_typeexpr(t: &TypeExpr) -> u8 {
    match t.name.as_str() {
        "int" => crate::ast::FLAG_GUARD_INT,
        "float" => crate::ast::FLAG_GUARD_FLOAT,
        "bool" => crate::ast::FLAG_GUARD_BOOL,
        "Option" | "option" if t.args.len() == 1 => {
            crate::ast::FLAG_GUARD_NULLABLE | guard_flags_of_typeexpr(&t.args[0])
        }
        _ => 0,
    }
}

struct LoopCtx {
    cont: Block,
    brk: Block,
    cp: Variable,
}

struct FEnv {
    vars: HashMap<String, Loc>,
    /// every pre-declared local variable (released once in the epilogue)
    locals_all: Vec<Variable>,
    ret_var: Variable,
    cp_var: Variable,
    cells_ptr: CVal,
    loop_stack: Vec<LoopCtx>,
    /// runtime typed-boundary guard bits for declared returns, including
    /// nullable scalar forms such as `int?`.
    ret_flags: u8,
    fn_name: String,
}

struct Emit<'a> {
    shared: &'a mut Shared,
    fns: &'a HashMap<usize, FuncId>,
    fn_name_of: &'a HashMap<usize, (String, usize)>,
    assigned: &'a HashSet<String>,
    res: &'a Resolution,
    tinfo: &'a crate::typecheck::TypeInfo,
    mods: &'a [ModMeta],
    /// true when emitting a unit pseudo-function (main or a module init):
    /// depth-0 declarations are bound as globals
    is_unit: bool,
}

impl<'a> Emit<'a> {
    // ---------- low level ----------
    fn rcall(
        &mut self,
        b: &mut FunctionBuilder,
        name: &'static str,
        nparams: usize,
        args: &[CVal],
    ) -> CResult<CVal> {
        let fid = self.shared.rt_id(name, nparams)?;
        let fr = self.shared.module.declare_func_in_func(fid, b.func);
        let call = b.ins().call(fr, args);
        Ok(b.inst_results(call)[0])
    }

    /// if the runtime error flag is set, branch to err_blk
    fn guard(&mut self, b: &mut FunctionBuilder, err_blk: Block) -> CResult<()> {
        let flag = self.rcall(b, "plix_err_flag", 0, &[])?;
        let cont = b.create_block();
        b.ins().brif(flag, err_blk, &[], cont, &[]);
        b.switch_to_block(cont);
        b.seal_block(cont);
        Ok(())
    }

    fn error_with_str(&mut self, b: &mut FunctionBuilder, msg: &str) -> CResult<()> {
        let (p, l) = self.str_ptr(b, msg)?;
        self.rcall(b, "plix_set_error", 2, &[p, l])?;
        Ok(())
    }

    /// conditional runtime error: when `cond` holds, set `msg` and jump to
    /// err_blk; otherwise fall through to a fresh ok block (left current).
    /// err_blk must only ever be reached with the error flag/message set.
    fn brif_err_msg(
        &mut self,
        b: &mut FunctionBuilder,
        cond: CVal,
        msg: &str,
        err_blk: Block,
    ) -> CResult<()> {
        let msg_blk = b.create_block();
        let ok_blk = b.create_block();
        b.ins().brif(cond, msg_blk, &[], ok_blk, &[]);
        b.switch_to_block(msg_blk);
        b.seal_block(msg_blk);
        self.error_with_str(b, msg)?;
        b.ins().jump(err_blk, &[]);
        b.switch_to_block(ok_blk);
        b.seal_block(ok_blk);
        Ok(())
    }

    fn str_ptr(&mut self, b: &mut FunctionBuilder, s: &str) -> CResult<(CVal, CVal)> {
        let did = self.shared.str_data(s)?;
        let gv = self.shared.module.declare_data_in_func(did, b.func);
        let ptr = b.ins().global_value(I64, gv);
        let len = b.ins().iconst(I64, s.len() as i64);
        Ok((ptr, len))
    }

    fn stack_args(&mut self, b: &mut FunctionBuilder, vals: &[CVal]) -> CResult<CVal> {
        let n = vals.len().max(1);
        let ss = b.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            (n * 8) as u32,
            3,
        ));
        for (i, v) in vals.iter().enumerate() {
            b.ins().stack_store(*v, ss, (i * 8) as i32);
        }
        Ok(b.ins().stack_addr(I64, ss, 0))
    }

    /// untag a boxed int (arithmetic shift right by 1)
    fn untag(&self, b: &mut FunctionBuilder, v: CVal) -> CVal {
        let one = b.ins().iconst(I64, 1);
        b.ins().sshr(v, one)
    }

    // =======================================================================
    // raw specialization (typed functions): unboxed int/float/bool emission
    // =======================================================================

    /// boxed V -> raw i64 with a hard type guard (typed boundary)
    fn unbox_int_guard(
        &mut self,
        b: &mut FunctionBuilder,
        v: CVal,
        err_blk: Block,
        what: &str,
    ) -> CResult<CVal> {
        let resv = b.declare_var(I64);
        let ok_blk = b.create_block();
        let bad_blk = b.create_block();
        let merge_blk = b.create_block();
        let one = b.ins().iconst(I64, 1);
        let odd = b.ins().band(v, one);
        b.ins().brif(odd, ok_blk, &[], bad_blk, &[]);
        b.switch_to_block(ok_blk);
        b.seal_block(ok_blk);
        let raw = self.untag(b, v);
        b.def_var(resv, raw);
        b.ins().jump(merge_blk, &[]);
        b.switch_to_block(bad_blk);
        b.seal_block(bad_blk);
        // rt owns the guard message (single source of truth, both backends)
        let msg = plixrt::heap::guard_msg_int(what);
        self.error_with_str(b, &msg)?;
        b.ins().jump(err_blk, &[]);
        b.switch_to_block(merge_blk);
        b.seal_block(merge_blk);
        Ok(b.use_var(resv))
    }

    /// boxed V -> raw f64 (int widens); hard error on anything else
    fn unbox_float_guard(
        &mut self,
        b: &mut FunctionBuilder,
        v: CVal,
        err_blk: Block,
        _what: &str,
    ) -> CResult<CVal> {
        // plix_as_f64 handles int widening and sets the error flag itself
        let fid = self.shared.rt_id_typed("plix_as_f64", &[I64], &[F64_TY])?;
        let fr = self.shared.module.declare_func_in_func(fid, b.func);
        let call = b.ins().call(fr, &[v]);
        let out = b.inst_results(call)[0];
        self.guard(b, err_blk)?;
        Ok(out)
    }

    /// raw value -> boxed V (fresh temp)
    fn box_raw(&mut self, b: &mut FunctionBuilder, rv: RawVal) -> CResult<CVal> {
        match rv {
            RawVal::I(v) => self.rcall(b, "plix_int", 1, &[v]),
            RawVal::F(v) => {
                let fid = self.shared.rt_id_typed("plix_box_f64", &[F64_TY], &[I64])?;
                let fr = self.shared.module.declare_func_in_func(fid, b.func);
                let call = b.ins().call(fr, &[v]);
                Ok(b.inst_results(call)[0])
            }
            RawVal::B(v) => Ok(self.tag_bool(b, v)),
        }
    }

    fn raw_want_of(loc: Loc) -> Option<Want> {
        match loc {
            Loc::RawInt(_) => Some(Want::Int),
            Loc::RawFloat(_) => Some(Want::Float),
            Loc::RawBool(_) => Some(Want::Bool),
            _ => None,
        }
    }

    /// static emission type: Some(raw) ONLY when emit_raw fully supports the
    /// expression; Box otherwise. Single source of truth for both.
    fn static_ty(&self, fe: &FEnv, e: &Expr) -> STy {
        let numeric = |a: STy, b: STy| -> bool {
            matches!(a, STy::Int | STy::Float) && matches!(b, STy::Int | STy::Float)
        };
        match &e.node {
            ExprKind::Int(i) => {
                if *i >= INT_MIN && *i <= INT_MAX {
                    STy::Int
                } else {
                    STy::Box
                }
            }
            ExprKind::Float(_) => STy::Float,
            ExprKind::Bool(_) => STy::Bool,
            ExprKind::Ident(name) => match fe.vars.get(name) {
                Some(Loc::RawInt(_)) => STy::Int,
                Some(Loc::RawFloat(_)) => STy::Float,
                Some(Loc::RawBool(_)) => STy::Bool,
                _ => STy::Box,
            },
            ExprKind::Unary(op, x) => {
                let sx = self.static_ty(fe, x);
                match op {
                    UnOp::Neg => match sx {
                        STy::Int => STy::Int,
                        STy::Float => STy::Float,
                        _ => STy::Box,
                    },
                    UnOp::BitNot => {
                        if sx == STy::Int {
                            STy::Int
                        } else {
                            STy::Box
                        }
                    }
                    UnOp::Not => {
                        if sx == STy::Bool {
                            STy::Bool
                        } else {
                            STy::Box
                        }
                    }
                }
            }
            ExprKind::Binary(op, a, b) => {
                let sa = self.static_ty(fe, a);
                let sb = self.static_ty(fe, b);
                match op {
                    BinOp::Add | BinOp::Sub | BinOp::Mul => {
                        if sa == STy::Int && sb == STy::Int {
                            STy::Int
                        } else if numeric(sa, sb) {
                            STy::Float
                        } else {
                            STy::Box
                        }
                    }
                    BinOp::Mod => {
                        if sa == STy::Int && sb == STy::Int {
                            STy::Int
                        } else {
                            STy::Box // float % falls back to the runtime
                        }
                    }
                    BinOp::Div => {
                        if numeric(sa, sb) {
                            STy::Float
                        } else {
                            STy::Box
                        }
                    }
                    BinOp::BAnd | BinOp::BOr | BinOp::BXor | BinOp::Shl | BinOp::Shr => {
                        if sa == STy::Int && sb == STy::Int {
                            STy::Int
                        } else {
                            STy::Box
                        }
                    }
                    BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                        if numeric(sa, sb) {
                            STy::Bool
                        } else {
                            STy::Box
                        }
                    }
                    BinOp::Eq | BinOp::Ne => {
                        if numeric(sa, sb) || (sa == STy::Bool && sb == STy::Bool) {
                            STy::Bool
                        } else {
                            STy::Box
                        }
                    }
                }
            }
            ExprKind::Logical(_, a, b) => {
                if self.static_ty(fe, a) == STy::Bool && self.static_ty(fe, b) == STy::Bool {
                    STy::Bool
                } else {
                    STy::Box
                }
            }
            ExprKind::Ternary(c, a, b) => {
                let _ = self.static_ty(fe, c); // cond may be anything truthy
                let sa = self.static_ty(fe, a);
                let sb = self.static_ty(fe, b);
                if sa == sb {
                    sa
                } else if numeric(sa, sb) {
                    STy::Float
                } else {
                    STy::Box
                }
            }
            ExprKind::Call(callee, _args) => {
                // direct-callable top-level fn with a typed numeric/bool return
                if let ExprKind::Ident(n) = &callee.node {
                    if !self.assigned.contains(n) {
                        if let Some(id) = self.find_stable_fn(n) {
                            if let Some(sig) = self.tinfo.fn_sigs.get(&id) {
                                return match sig.ret.as_ref().map(|t| match t {
                                    crate::typecheck::Ty::Int => STy::Int,
                                    crate::typecheck::Ty::Float => STy::Float,
                                    crate::typecheck::Ty::Bool => STy::Bool,
                                    _ => STy::Box,
                                }) {
                                    Some(s) => s,
                                    None => STy::Box,
                                };
                            }
                        }
                    }
                }
                STy::Box
            }
            _ => STy::Box,
        }
    }

    /// emit an expression in raw form; the caller must have established
    /// static_ty(e) != Box. Returns None only defensively.
    fn emit_raw(
        &mut self,
        b: &mut FunctionBuilder,
        fe: &mut FEnv,
        e: &Expr,
        epilogue: Block,
        err_blk: Block,
    ) -> CResult<Option<RawVal>> {
        let numeric = |a: STy, b: STy| -> bool {
            matches!(a, STy::Int | STy::Float) && matches!(b, STy::Int | STy::Float)
        };
        match &e.node {
            ExprKind::Int(i) => {
                let v = b.ins().iconst(I64, *i);
                Ok(Some(RawVal::I(v)))
            }
            ExprKind::Float(f) => {
                let v = b.ins().f64const(*f);
                Ok(Some(RawVal::F(v)))
            }
            ExprKind::Bool(x) => {
                let v = b.ins().iconst(I64, if *x { 1 } else { 0 });
                Ok(Some(RawVal::B(v)))
            }
            ExprKind::Ident(name) => match fe.vars.get(name) {
                Some(Loc::RawInt(v)) => Ok(Some(RawVal::I(b.use_var(*v)))),
                Some(Loc::RawFloat(v)) => Ok(Some(RawVal::F(b.use_var(*v)))),
                Some(Loc::RawBool(v)) => Ok(Some(RawVal::B(b.use_var(*v)))),
                _ => Ok(None),
            },
            ExprKind::Unary(op, x) => match op {
                UnOp::Neg => {
                    let rv = self.emit_raw(b, fe, x, epilogue, err_blk)?;
                    match rv {
                        Some(RawVal::I(v)) => {
                            // int negation: i64::MIN cannot occur (62-bit domain),
                            // guard anyway for soundness
                            let minv = b.ins().iconst(I64, i64::MIN);
                            let is_min = b.ins().icmp(IntCC::Equal, v, minv);
                            self.brif_err_msg(b, is_min, "int negation overflow", err_blk)?;
                            let r = b.ins().ineg(v);
                            Ok(Some(RawVal::I(r)))
                        }
                        Some(RawVal::F(v)) => Ok(Some(RawVal::F(b.ins().fneg(v)))),
                        _ => Ok(None),
                    }
                }
                UnOp::BitNot => match self.emit_raw(b, fe, x, epilogue, err_blk)? {
                    Some(RawVal::I(v)) => Ok(Some(RawVal::I(b.ins().bnot(v)))),
                    _ => Ok(None),
                },
                UnOp::Not => match self.emit_raw(b, fe, x, epilogue, err_blk)? {
                    Some(RawVal::B(v)) => {
                        let one = b.ins().iconst(I64, 1);
                        Ok(Some(RawVal::B(b.ins().bxor(v, one))))
                    }
                    _ => Ok(None),
                },
            },
            ExprKind::Binary(op, a, b2) => {
                let sa = self.static_ty(fe, a);
                let sb = self.static_ty(fe, b2);
                match op {
                    BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Mod
                        if sa == STy::Int && sb == STy::Int =>
                    {
                        let ra = self.emit_raw(b, fe, a, epilogue, err_blk)?;
                        let rb = self.emit_raw(b, fe, b2, epilogue, err_blk)?;
                        let (RawVal::I(x), RawVal::I(y)) = (ra.unwrap(), rb.unwrap()) else {
                            return Ok(None);
                        };
                        let r = self.int_checked(b, *op, x, y, err_blk)?;
                        Ok(Some(RawVal::I(r)))
                    }
                    BinOp::Add | BinOp::Sub | BinOp::Mul if numeric(sa, sb) => {
                        let x = self.emit_raw_f64(b, fe, a, epilogue, err_blk)?;
                        let y = self.emit_raw_f64(b, fe, b2, epilogue, err_blk)?;
                        let r = match op {
                            BinOp::Add => b.ins().fadd(x, y),
                            BinOp::Sub => b.ins().fsub(x, y),
                            _ => b.ins().fmul(x, y),
                        };
                        Ok(Some(RawVal::F(r)))
                    }
                    BinOp::Div if numeric(sa, sb) => {
                        let x = self.emit_raw_f64(b, fe, a, epilogue, err_blk)?;
                        let y = self.emit_raw_f64(b, fe, b2, epilogue, err_blk)?;
                        // division by zero is a runtime error (language semantics)
                        let zero = b.ins().f64const(0.0);
                        let is_zero =
                            b.ins()
                                .fcmp(cranelift_codegen::ir::condcodes::FloatCC::Equal, y, zero);
                        self.brif_err_msg(b, is_zero, "division by zero", err_blk)?;
                        let r = b.ins().fdiv(x, y);
                        Ok(Some(RawVal::F(r)))
                    }
                    BinOp::BAnd | BinOp::BOr | BinOp::BXor if sa == STy::Int && sb == STy::Int => {
                        let ra = self.emit_raw(b, fe, a, epilogue, err_blk)?;
                        let rb = self.emit_raw(b, fe, b2, epilogue, err_blk)?;
                        let (RawVal::I(x), RawVal::I(y)) = (ra.unwrap(), rb.unwrap()) else {
                            return Ok(None);
                        };
                        let r = match op {
                            BinOp::BAnd => b.ins().band(x, y),
                            BinOp::BOr => b.ins().bor(x, y),
                            _ => b.ins().bxor(x, y),
                        };
                        Ok(Some(RawVal::I(r)))
                    }
                    BinOp::Shl | BinOp::Shr if sa == STy::Int && sb == STy::Int => {
                        let ra = self.emit_raw(b, fe, a, epilogue, err_blk)?;
                        let rb = self.emit_raw(b, fe, b2, epilogue, err_blk)?;
                        let (RawVal::I(x), RawVal::I(y)) = (ra.unwrap(), rb.unwrap()) else {
                            return Ok(None);
                        };
                        let r = self.int_shift_checked(b, *op, x, y, err_blk)?;
                        Ok(Some(RawVal::I(r)))
                    }
                    BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge if numeric(sa, sb) => {
                        let r = self.raw_cmp(b, fe, a, b2, *op, epilogue, err_blk)?;
                        Ok(Some(RawVal::B(r)))
                    }
                    BinOp::Eq | BinOp::Ne => {
                        let r = self.raw_eq(b, fe, a, b2, *op, epilogue, err_blk, sa, sb)?;
                        Ok(Some(RawVal::B(r)))
                    }
                    _ => Ok(None),
                }
            }
            ExprKind::Logical(lop, a, b2) => {
                let ra = self.emit_raw(b, fe, a, epilogue, err_blk)?;
                let Some(RawVal::B(x)) = ra else {
                    return Ok(None);
                };
                let resv = b.declare_var(I64);
                let rhs_blk = b.create_block();
                let done_blk = b.create_block();
                match lop {
                    LogicalOp::And => {
                        b.ins().brif(x, rhs_blk, &[], done_blk, &[]);
                    }
                    LogicalOp::Or => {
                        b.ins().brif(x, done_blk, &[], rhs_blk, &[]);
                    }
                }
                b.def_var(resv, x);
                b.switch_to_block(rhs_blk);
                b.seal_block(rhs_blk);
                let rb = self.emit_raw(b, fe, b2, epilogue, err_blk)?;
                let Some(RawVal::B(y)) = rb else {
                    return Err("internal: logical rhs not raw bool".into());
                };
                b.def_var(resv, y);
                b.ins().jump(done_blk, &[]);
                b.switch_to_block(done_blk);
                b.seal_block(done_blk);
                Ok(Some(RawVal::B(b.use_var(resv))))
            }
            ExprKind::Ternary(c, a, b2) => {
                let join = self.static_ty(fe, e);
                if join == STy::Box {
                    return Ok(None);
                }
                let flag = self.emit_cond(b, fe, c, epilogue, err_blk)?;
                let a_blk = b.create_block();
                let b_blk = b.create_block();
                let done_blk = b.create_block();
                b.ins().brif(flag, a_blk, &[], b_blk, &[]);
                let rawty = match join {
                    STy::Int => I64,
                    STy::Float => F64_TY,
                    _ => I64,
                };
                let resv = b.declare_var(rawty);
                b.switch_to_block(a_blk);
                b.seal_block(a_blk);
                let ra = self.emit_raw_as(b, fe, a, join, epilogue, err_blk)?;
                b.def_var(resv, ra);
                b.ins().jump(done_blk, &[]);
                b.switch_to_block(b_blk);
                b.seal_block(b_blk);
                let rb = self.emit_raw_as(b, fe, b2, join, epilogue, err_blk)?;
                b.def_var(resv, rb);
                b.ins().jump(done_blk, &[]);
                b.switch_to_block(done_blk);
                b.seal_block(done_blk);
                let out = b.use_var(resv);
                Ok(Some(match join {
                    STy::Int => RawVal::I(out),
                    STy::Float => RawVal::F(out),
                    _ => RawVal::B(out),
                }))
            }
            ExprKind::Call(callee, args) => {
                // direct, stable, fully-typed function: boxed ABI + guarded
                // unbox of the declared return type
                let mut direct: Option<(usize, STy)> = None;
                if let ExprKind::Ident(n) = &callee.node {
                    if !self.assigned.contains(n) {
                        if let Some(id) = self.find_stable_fn(n) {
                            if let Some(sig) = self.tinfo.fn_sigs.get(&id) {
                                let sty = sig.ret.as_ref().map(|t| match t {
                                    crate::typecheck::Ty::Int => STy::Int,
                                    crate::typecheck::Ty::Float => STy::Float,
                                    crate::typecheck::Ty::Bool => STy::Bool,
                                    _ => STy::Box,
                                });
                                if let Some(t) = sty {
                                    if t != STy::Box {
                                        direct = Some((id, t));
                                    }
                                }
                            }
                        }
                    }
                }
                let Some((id, sty)) = direct else {
                    return Ok(None);
                };
                let mut vals = Vec::with_capacity(args.len());
                for a in args {
                    let v = self.expr(b, fe, a, epilogue, err_blk)?;
                    self.guard(b, err_blk)?;
                    vals.push(v);
                }
                let addr = self.stack_args(b, &vals)?;
                let n = b.ins().iconst(I64, vals.len() as i64);
                let fidr = self.fns[&id];
                let nullv = b.ins().iconst(I64, TNULL);
                let fr = self.shared.module.declare_func_in_func(fidr, b.func);
                let call = b.ins().call(fr, &[nullv, addr, n]);
                let ret = b.inst_results(call)[0];
                self.guard(b, err_blk)?;
                match sty {
                    STy::Int => {
                        let raw = self.unbox_int_guard(
                            b,
                            ret,
                            err_blk,
                            &format!("return of typed function"),
                        )?;
                        Ok(Some(RawVal::I(raw)))
                    }
                    STy::Float => {
                        let raw = self.unbox_float_guard(b, ret, err_blk, "return value")?;
                        Ok(Some(RawVal::F(raw)))
                    }
                    STy::Bool => {
                        let t = self.rcall(b, "plix_truthy", 1, &[ret])?;
                        Ok(Some(RawVal::B(t)))
                    }
                    STy::Box => Ok(None),
                }
            }
            _ => Ok(None),
        }
    }

    /// emit an expression as a specific raw type (widening int->float when
    /// needed); expects static compatibility
    fn emit_raw_as(
        &mut self,
        b: &mut FunctionBuilder,
        fe: &mut FEnv,
        e: &Expr,
        want: STy,
        epilogue: Block,
        err_blk: Block,
    ) -> CResult<CVal> {
        let rv = self.emit_raw(b, fe, e, epilogue, err_blk)?;
        match (want, rv) {
            (STy::Int, Some(RawVal::I(v))) => Ok(v),
            (STy::Float, Some(RawVal::F(v))) => Ok(v),
            (STy::Float, Some(RawVal::I(v))) => Ok(b.ins().fcvt_from_sint(F64_TY, v)),
            (STy::Bool, Some(RawVal::B(v))) => Ok(v),
            _ => Err("internal: emit_raw_as type mismatch".into()),
        }
    }

    /// emit as raw f64 (int operands widen)
    fn emit_raw_f64(
        &mut self,
        b: &mut FunctionBuilder,
        fe: &mut FEnv,
        e: &Expr,
        epilogue: Block,
        err_blk: Block,
    ) -> CResult<CVal> {
        self.emit_raw_as(b, fe, e, STy::Float, epilogue, err_blk)
    }

    /// int add/sub/mul/mod with overflow => hard runtime error (typed int
    /// arithmetic is strict — like Rust's debug builds; dynamic code keeps
    /// the float-promotion semantics)
    fn int_checked(
        &mut self,
        b: &mut FunctionBuilder,
        op: BinOp,
        x: CVal,
        y: CVal,
        err_blk: Block,
    ) -> CResult<CVal> {
        match op {
            BinOp::Add | BinOp::Sub => {
                let r = match op {
                    BinOp::Add => b.ins().iadd(x, y),
                    _ => b.ins().isub(x, y),
                };
                // signed overflow: (r^x)&(r^y) < 0 for add; (x^y)&(x^r) < 0 for sub
                let (t1, t2) = match op {
                    BinOp::Add => (b.ins().bxor(r, x), b.ins().bxor(r, y)),
                    _ => (b.ins().bxor(x, y), b.ins().bxor(x, r)),
                };
                let m = b.ins().band(t1, t2);
                let zero = b.ins().iconst(I64, 0);
                let ov = b.ins().icmp(IntCC::SignedLessThan, m, zero);
                let opname = if op == BinOp::Add {
                    "addition"
                } else {
                    "subtraction"
                };
                self.brif_err_msg(
                    b,
                    ov,
                    &format!("integer overflow in typed int {opname}"),
                    err_blk,
                )?;
                // typed ints live in the same 62-bit domain as dynamic ints:
                // beyond it is strict overflow, never a silent float
                self.int_range_guard(b, r, opname, err_blk)?;
                Ok(r)
            }
            BinOp::Mul => {
                let r = b.ins().imul(x, y);
                // overflow iff NOT (x == 0 | (x == -1 & y != IMIN) | r/x == y)
                // NB: sdiv must never execute with x in {0, -1} (hardware trap
                // at IMIN / -1), so the checks live in separate blocks.
                let zero = b.ins().iconst(I64, 0);
                let neg1 = b.ins().iconst(I64, -1);
                let imin = b.ins().iconst(I64, i64::MIN);
                let x_zero = b.ins().icmp(IntCC::Equal, x, zero);
                let x_neg1 = b.ins().icmp(IntCC::Equal, x, neg1);
                let y_imin = b.ins().icmp(IntCC::Equal, y, imin);
                let done_blk = b.create_block();
                let chk_neg1_blk = b.create_block();
                let special_blk = b.create_block();
                let general_blk = b.create_block();
                b.ins().brif(x_zero, done_blk, &[], chk_neg1_blk, &[]);
                b.switch_to_block(chk_neg1_blk);
                b.seal_block(chk_neg1_blk);
                b.ins().brif(x_neg1, special_blk, &[], general_blk, &[]);
                b.switch_to_block(special_blk);
                b.seal_block(special_blk);
                // x == -1: r == -y; overflow only when y == IMIN
                self.brif_err_msg(
                    b,
                    y_imin,
                    "integer overflow in typed int multiplication",
                    err_blk,
                )?;
                b.ins().jump(done_blk, &[]);
                b.switch_to_block(general_blk);
                b.seal_block(general_blk);
                let q = b.ins().sdiv(r, x);
                let bad = b.ins().icmp(IntCC::NotEqual, q, y);
                self.brif_err_msg(
                    b,
                    bad,
                    "integer overflow in typed int multiplication",
                    err_blk,
                )?;
                b.ins().jump(done_blk, &[]);
                b.switch_to_block(done_blk);
                b.seal_block(done_blk);
                self.int_range_guard(b, r, "multiplication", err_blk)?;
                Ok(r)
            }
            BinOp::Mod => {
                let zero = b.ins().iconst(I64, 0);
                let y_zero = b.ins().icmp(IntCC::Equal, y, zero);
                self.brif_err_msg(b, y_zero, "remainder by zero", err_blk)?;
                // srem traps at INT_MIN % -1 on x86; that case yields 0
                let neg1 = b.ins().iconst(I64, -1);
                let imin = b.ins().iconst(I64, i64::MIN);
                let x_imin = b.ins().icmp(IntCC::Equal, x, imin);
                let y_neg1 = b.ins().icmp(IntCC::Equal, y, neg1);
                let trap = b.ins().band(x_imin, y_neg1);
                let trap_blk = b.create_block();
                let ok2_blk = b.create_block();
                let merge_blk = b.create_block();
                let resv = b.declare_var(I64);
                b.ins().brif(trap, trap_blk, &[], ok2_blk, &[]);
                b.switch_to_block(trap_blk);
                b.seal_block(trap_blk);
                b.def_var(resv, zero);
                b.ins().jump(merge_blk, &[]);
                b.switch_to_block(ok2_blk);
                b.seal_block(ok2_blk);
                let r = b.ins().srem(x, y);
                b.def_var(resv, r);
                b.ins().jump(merge_blk, &[]);
                b.switch_to_block(merge_blk);
                b.seal_block(merge_blk);
                Ok(b.use_var(resv))
            }
            _ => Err("internal: int_checked op".into()),
        }
    }

    /// typed int arithmetic is confined to the dynamic 62-bit domain:
    /// exceeding it is a strict overflow error (interpreter parity)
    fn int_range_guard(
        &mut self,
        b: &mut FunctionBuilder,
        r: CVal,
        what: &str,
        err_blk: Block,
    ) -> CResult<()> {
        let lo = b.ins().iconst(I64, INT_MIN);
        let hi = b.ins().iconst(I64, INT_MAX);
        let too_lo = b.ins().icmp(IntCC::SignedLessThan, r, lo);
        let too_hi = b.ins().icmp(IntCC::SignedGreaterThan, r, hi);
        let bad = b.ins().bor(too_lo, too_hi);
        self.brif_err_msg(
            b,
            bad,
            &format!("integer overflow in typed int {}", what),
            err_blk,
        )
    }

    fn int_shift_checked(
        &mut self,
        b: &mut FunctionBuilder,
        op: BinOp,
        x: CVal,
        y: CVal,
        err_blk: Block,
    ) -> CResult<CVal> {
        // negative count => error; >= 62 => 0 (logical) or 0/-1 (arith shift,
        // matching the interpreter's tagged-int semantics)
        let zero = b.ins().iconst(I64, 0);
        let neg = b.ins().icmp(IntCC::SignedLessThan, y, zero);
        self.brif_err_msg(b, neg, "negative shift count", err_blk)?;
        let cap = b.ins().iconst(I64, 62);
        let too_big = b.ins().icmp(IntCC::SignedGreaterThanOrEqual, y, cap);
        let big_blk = b.create_block();
        let norm_blk = b.create_block();
        let merge_blk = b.create_block();
        let resv = b.declare_var(I64);
        b.ins().brif(too_big, big_blk, &[], norm_blk, &[]);
        b.switch_to_block(big_blk);
        b.seal_block(big_blk);
        let big_res = match op {
            BinOp::Shl => b.ins().iconst(I64, 0),
            _ => {
                // shr: sign-propagating saturation
                let neg_x = b.ins().icmp(IntCC::SignedLessThan, x, zero);
                let m1 = b.ins().iconst(I64, -1);
                b.ins().select(neg_x, m1, zero)
            }
        };
        b.def_var(resv, big_res);
        b.ins().jump(merge_blk, &[]);
        b.switch_to_block(norm_blk);
        b.seal_block(norm_blk);
        let r = match op {
            BinOp::Shl => b.ins().ishl(x, y),
            _ => b.ins().sshr(x, y),
        };
        b.def_var(resv, r);
        b.ins().jump(merge_blk, &[]);
        b.switch_to_block(merge_blk);
        b.seal_block(merge_blk);
        Ok(b.use_var(resv))
    }

    /// raw numeric comparison -> 0/1
    fn raw_cmp(
        &mut self,
        b: &mut FunctionBuilder,
        fe: &mut FEnv,
        a: &Expr,
        c: &Expr,
        op: BinOp,
        epilogue: Block,
        err_blk: Block,
    ) -> CResult<CVal> {
        let sa = self.static_ty(fe, a);
        let sb = self.static_ty(fe, c);
        let cc = match op {
            BinOp::Lt => IntCC::SignedLessThan,
            BinOp::Le => IntCC::SignedLessThanOrEqual,
            BinOp::Gt => IntCC::SignedGreaterThan,
            BinOp::Ge => IntCC::SignedGreaterThanOrEqual,
            _ => return Err("internal: raw_cmp op".into()),
        };
        let fcc = match op {
            BinOp::Lt => cranelift_codegen::ir::condcodes::FloatCC::LessThan,
            BinOp::Le => cranelift_codegen::ir::condcodes::FloatCC::LessThanOrEqual,
            BinOp::Gt => cranelift_codegen::ir::condcodes::FloatCC::GreaterThan,
            BinOp::Ge => cranelift_codegen::ir::condcodes::FloatCC::GreaterThanOrEqual,
            _ => unreachable!(),
        };
        if sa == STy::Int && sb == STy::Int {
            let ra = self.emit_raw(b, fe, a, epilogue, err_blk)?;
            let rb = self.emit_raw(b, fe, c, epilogue, err_blk)?;
            let (Some(RawVal::I(x)), Some(RawVal::I(y))) = (ra, rb) else {
                return Err("internal: raw_cmp ints".into());
            };
            let i8v = b.ins().icmp(cc, x, y);
            Ok(b.ins().uextend(I64, i8v))
        } else {
            let x = self.emit_raw_f64(b, fe, a, epilogue, err_blk)?;
            let y = self.emit_raw_f64(b, fe, c, epilogue, err_blk)?;
            let i8v = b.ins().fcmp(fcc, x, y);
            Ok(b.ins().uextend(I64, i8v))
        }
    }

    /// raw equality for provably numeric/bool operands -> 0/1
    fn raw_eq(
        &mut self,
        b: &mut FunctionBuilder,
        fe: &mut FEnv,
        a: &Expr,
        c: &Expr,
        op: BinOp,
        epilogue: Block,
        err_blk: Block,
        sa: STy,
        sb: STy,
    ) -> CResult<CVal> {
        let numeric = |t: STy| matches!(t, STy::Int | STy::Float);
        let eq = match op {
            BinOp::Eq => true,
            _ => false,
        };
        let i8v = if sa == STy::Int && sb == STy::Int {
            let ra = self.emit_raw(b, fe, a, epilogue, err_blk)?;
            let rb = self.emit_raw(b, fe, c, epilogue, err_blk)?;
            let (Some(RawVal::I(x)), Some(RawVal::I(y))) = (ra, rb) else {
                return Err("internal: raw_eq ints".into());
            };
            b.ins()
                .icmp(if eq { IntCC::Equal } else { IntCC::NotEqual }, x, y)
        } else if sa == STy::Bool && sb == STy::Bool {
            let ra = self.emit_raw(b, fe, a, epilogue, err_blk)?;
            let rb = self.emit_raw(b, fe, c, epilogue, err_blk)?;
            let (Some(RawVal::B(x)), Some(RawVal::B(y))) = (ra, rb) else {
                return Err("internal: raw_eq bools".into());
            };
            b.ins()
                .icmp(if eq { IntCC::Equal } else { IntCC::NotEqual }, x, y)
        } else if numeric(sa) && numeric(sb) {
            let x = self.emit_raw_f64(b, fe, a, epilogue, err_blk)?;
            let y = self.emit_raw_f64(b, fe, c, epilogue, err_blk)?;
            b.ins().fcmp(
                if eq {
                    cranelift_codegen::ir::condcodes::FloatCC::Equal
                } else {
                    cranelift_codegen::ir::condcodes::FloatCC::NotEqual
                },
                x,
                y,
            )
        } else {
            return Err("internal: raw_eq unsupported".into());
        };
        Ok(b.ins().uextend(I64, i8v))
    }

    /// compound assignment on a raw local (x += e etc.)
    fn raw_compound(
        &mut self,
        b: &mut FunctionBuilder,
        fe: &mut FEnv,
        loc: Loc,
        want: Want,
        op: AssignOp,
        value: &Expr,
        epilogue: Block,
        err_blk: Block,
    ) -> CResult<CVal> {
        let raw_var = match loc {
            Loc::RawInt(v) | Loc::RawFloat(v) | Loc::RawBool(v) => v,
            _ => unreachable!(),
        };
        let old = b.use_var(raw_var);
        match want {
            Want::Int => match op {
                AssignOp::Add | AssignOp::Sub | AssignOp::Mul | AssignOp::Mod => {
                    let bop = match op {
                        AssignOp::Add => BinOp::Add,
                        AssignOp::Sub => BinOp::Sub,
                        AssignOp::Mul => BinOp::Mul,
                        AssignOp::Mod => BinOp::Mod,
                        AssignOp::Div | AssignOp::Eq => unreachable!(),
                    };
                    // int compound can overflow (strict error); float rhs is
                    // rejected by the checker for int slots
                    let st = self.static_ty(fe, value);
                    if st == STy::Int {
                        let rhs = self
                            .emit_raw(b, fe, value, epilogue, err_blk)?
                            .ok_or("internal: raw compound rhs")?;
                        let RawVal::I(ri) = rhs else {
                            return Err("internal: raw compound int rhs".into());
                        };
                        self.int_checked(b, bop, old, ri, err_blk)
                    } else {
                        let boxed = self.expr(b, fe, value, epilogue, err_blk)?;
                        self.guard(b, err_blk)?;
                        let ri = self.unbox_int_guard(b, boxed, err_blk, "compound assignment")?;
                        self.int_checked(b, bop, old, ri, err_blk)
                    }
                }
                AssignOp::Div => {
                    // /= always yields float; into a typed int slot that's an
                    // error statically — this path is unreachable if the
                    // checker ran, but fail hard instead of corrupting
                    self.error_with_str(
                        b,
                        "cannot store float division result into a typed int slot",
                    )?;
                    b.ins().jump(err_blk, &[]);
                    let dead = b.create_block();
                    b.switch_to_block(dead);
                    b.seal_block(dead);
                    Ok(b.ins().iconst(I64, 0))
                }
                AssignOp::Eq => unreachable!(),
            },
            Want::Float => {
                let rhs = match self.static_ty(fe, value) {
                    STy::Int | STy::Float => self.emit_raw_f64(b, fe, value, epilogue, err_blk)?,
                    _ => {
                        let boxed = self.expr(b, fe, value, epilogue, err_blk)?;
                        self.guard(b, err_blk)?;
                        self.unbox_float_guard(b, boxed, err_blk, "compound assignment")?
                    }
                };
                match op {
                    AssignOp::Add => Ok(b.ins().fadd(old, rhs)),
                    AssignOp::Sub => Ok(b.ins().fsub(old, rhs)),
                    AssignOp::Mul => Ok(b.ins().fmul(old, rhs)),
                    AssignOp::Div => {
                        let zero = b.ins().f64const(0.0);
                        let is_zero = b.ins().fcmp(
                            cranelift_codegen::ir::condcodes::FloatCC::Equal,
                            rhs,
                            zero,
                        );
                        self.brif_err_msg(b, is_zero, "division by zero", err_blk)?;
                        Ok(b.ins().fdiv(old, rhs))
                    }
                    AssignOp::Mod => {
                        // float % via the runtime (exact v0.2 semantics)
                        let ob = self.box_raw(b, RawVal::F(old))?;
                        let rb = self.box_raw(b, RawVal::F(rhs))?;
                        let r = self.rcall(b, "plix_rem", 2, &[ob, rb])?;
                        self.guard(b, err_blk)?;
                        self.unbox_float_guard(b, r, err_blk, "compound %")
                    }
                    AssignOp::Eq => unreachable!(),
                }
            }
            Want::Bool => Err("internal: compound assignment to bool (checker bug)".into()),
        }
    }

    /// condition for if/while/ternary: raw 0/1 when statically boolean,
    /// otherwise the boxed truthy path
    fn emit_cond(
        &mut self,
        b: &mut FunctionBuilder,
        fe: &mut FEnv,
        e: &Expr,
        epilogue: Block,
        err_blk: Block,
    ) -> CResult<CVal> {
        if self.static_ty(fe, e) == STy::Bool {
            if let Some(RawVal::B(v)) = self.emit_raw(b, fe, e, epilogue, err_blk)? {
                return Ok(v);
            }
        }
        let c = self.expr(b, fe, e, epilogue, err_blk)?;
        self.guard(b, err_blk)?;
        self.rcall(b, "plix_truthy", 1, &[c])
    }

    /// produce a raw-typed value for `e` (for a raw local store): raw path
    /// when statically provable, else boxed path + boundary guard
    fn emit_typed(
        &mut self,
        b: &mut FunctionBuilder,
        fe: &mut FEnv,
        e: &Expr,
        want: Want,
        err_blk: Block,
        epilogue: Block,
        what: &str,
    ) -> CResult<CVal> {
        let want_sty = match want {
            Want::Int => STy::Int,
            Want::Float => STy::Float,
            Want::Bool => STy::Bool,
        };
        let st = self.static_ty(fe, e);
        let raw_ok =
            st != STy::Box && (st == want_sty || (want_sty == STy::Float && st == STy::Int));
        if raw_ok {
            if let Some(rv) = self.emit_raw(b, fe, e, epilogue, err_blk)? {
                return match (want, rv) {
                    (Want::Int, RawVal::I(v)) => Ok(v),
                    (Want::Float, RawVal::F(v)) => Ok(v),
                    (Want::Float, RawVal::I(v)) => Ok(b.ins().fcvt_from_sint(F64_TY, v)),
                    (Want::Bool, RawVal::B(v)) => Ok(v),
                    (Want::Int, RawVal::F(_)) => {
                        Err("internal: float raw into int slot (checker bug)".into())
                    }
                    _ => Err("internal: emit_typed representation mismatch".into()),
                };
            }
        }
        // generic path: boxed V + boundary guard
        let v = self.expr(b, fe, e, epilogue, err_blk)?;
        self.guard(b, err_blk)?;
        match want {
            Want::Int => self.unbox_int_guard(b, v, err_blk, what),
            Want::Float => self.unbox_float_guard(b, v, err_blk, what),
            Want::Bool => self.rcall(b, "plix_truthy", 1, &[v]),
        }
    }

    // ---------- variable access ----------
    fn lookup(&self, fe: &FEnv, name: &str, line: u32) -> CResult<Loc> {
        if let Some(&l) = fe.vars.get(name) {
            return Ok(l);
        }
        if let Some(&g) = self.res.globals.get(name) {
            return Ok(Loc::Global(g));
        }
        Err(format!("unresolved name \"{}\" (near line {})", name, line))
    }

    /// load a value as a fresh arena-owned temp
    fn load_loc(&mut self, b: &mut FunctionBuilder, fe: &mut FEnv, loc: Loc) -> CResult<CVal> {
        match loc {
            Loc::Local(v) => {
                let raw = b.use_var(v);
                self.rcall(b, "plix_var_use", 1, &[raw])
            }
            Loc::Cell(cv) => {
                let cell = b.use_var(cv);
                self.rcall(b, "plix_cell_get", 1, &[cell])
            }
            Loc::Free(i) => {
                let cell = b
                    .ins()
                    .load(I64, MemFlags::trusted(), fe.cells_ptr, (i * 8) as i32);
                self.rcall(b, "plix_cell_get", 1, &[cell])
            }
            Loc::Global(g) => {
                let idx = b.ins().iconst(I64, g as i64);
                self.rcall(b, "plix_global_get", 1, &[idx])
            }
            // raw locals are boxed on demand (boxing always yields either a
            // tagged immediate or a fresh arena-owned float — temp semantics)
            Loc::RawInt(v) => {
                let raw = b.use_var(v);
                self.rcall(b, "plix_int", 1, &[raw])
            }
            Loc::RawFloat(v) => {
                let raw = b.use_var(v);
                let fid = self.shared.rt_id_typed("plix_box_f64", &[F64_TY], &[I64])?;
                let fr = self.shared.module.declare_func_in_func(fid, b.func);
                let call = b.ins().call(fr, &[raw]);
                Ok(b.inst_results(call)[0])
            }
            Loc::RawBool(v) => {
                let raw = b.use_var(v);
                Ok(self.tag_bool(b, raw))
            }
        }
    }

    /// store into an existing variable (retain new, release old)
    fn assign_loc(
        &mut self,
        b: &mut FunctionBuilder,
        fe: &mut FEnv,
        loc: Loc,
        newv: CVal,
    ) -> CResult<()> {
        match loc {
            Loc::Local(v) => {
                let old = b.use_var(v);
                self.rcall(b, "plix_retain", 1, &[newv])?;
                self.rcall(b, "plix_release", 1, &[old])?;
                b.def_var(v, newv);
                Ok(())
            }
            Loc::Cell(cv) => {
                let cell = b.use_var(cv);
                self.rcall(b, "plix_cell_set", 2, &[cell, newv])?;
                Ok(())
            }
            Loc::Free(i) => {
                let cell = b
                    .ins()
                    .load(I64, MemFlags::trusted(), fe.cells_ptr, (i * 8) as i32);
                self.rcall(b, "plix_cell_set", 2, &[cell, newv])?;
                Ok(())
            }
            Loc::Global(g) => {
                let idx = b.ins().iconst(I64, g as i64);
                self.rcall(b, "plix_global_set", 2, &[idx, newv])?;
                Ok(())
            }
            _ => Err(
                "internal: assign_loc expects a boxed V; raw locals are handled by raw store paths"
                    .into(),
            ),
        }
    }

    /// store a boxed V into a raw local with a runtime guard (the typed
    /// boundary: a wrong runtime type raises, never a silent bad read)
    /// boundary guard for a typed slot held as a *boxed* location (global,
    /// captured cell, or otherwise unrawified local): identical checks and
    /// canonical representation to the interpreter's guard_typed
    fn guard_boxed(
        &mut self,
        b: &mut FunctionBuilder,
        flags: u8,
        v: CVal,
        err_blk: Block,
        what: &str,
    ) -> CResult<CVal> {
        use crate::ast::{FLAG_GUARD_BOOL, FLAG_GUARD_FLOAT, FLAG_GUARD_INT, FLAG_GUARD_NULLABLE};
        let nullable = flags & FLAG_GUARD_NULLABLE != 0;

        if flags & FLAG_GUARD_INT != 0 {
            if nullable {
                let resv = b.declare_var(I64);
                let null_blk = b.create_block();
                let check_blk = b.create_block();
                let done_blk = b.create_block();
                let n = b.ins().iconst(I64, TNULL);
                let is_null = b.ins().icmp(IntCC::Equal, v, n);
                b.ins().brif(is_null, null_blk, &[], check_blk, &[]);
                b.switch_to_block(null_blk);
                b.seal_block(null_blk);
                b.def_var(resv, v);
                b.ins().jump(done_blk, &[]);
                b.switch_to_block(check_blk);
                b.seal_block(check_blk);
                let raw = self.unbox_int_guard(b, v, err_blk, what)?;
                let boxed = self.rcall(b, "plix_int", 1, &[raw])?;
                b.def_var(resv, boxed);
                b.ins().jump(done_blk, &[]);
                b.switch_to_block(done_blk);
                b.seal_block(done_blk);
                return Ok(b.use_var(resv));
            }
            let raw = self.unbox_int_guard(b, v, err_blk, what)?;
            return self.rcall(b, "plix_int", 1, &[raw]);
        }
        if flags & FLAG_GUARD_FLOAT != 0 {
            if nullable {
                let resv = b.declare_var(I64);
                let null_blk = b.create_block();
                let check_blk = b.create_block();
                let done_blk = b.create_block();
                let n = b.ins().iconst(I64, TNULL);
                let is_null = b.ins().icmp(IntCC::Equal, v, n);
                b.ins().brif(is_null, null_blk, &[], check_blk, &[]);
                b.switch_to_block(null_blk);
                b.seal_block(null_blk);
                b.def_var(resv, v);
                b.ins().jump(done_blk, &[]);
                b.switch_to_block(check_blk);
                b.seal_block(check_blk);
                let raw = self.unbox_float_guard(b, v, err_blk, what)?;
                let boxed = self.box_raw(b, RawVal::F(raw))?;
                b.def_var(resv, boxed);
                b.ins().jump(done_blk, &[]);
                b.switch_to_block(done_blk);
                b.seal_block(done_blk);
                return Ok(b.use_var(resv));
            }
            let raw = self.unbox_float_guard(b, v, err_blk, what)?;
            return self.box_raw(b, RawVal::F(raw));
        }
        if flags & FLAG_GUARD_BOOL != 0 {
            if nullable {
                let resv = b.declare_var(I64);
                let null_blk = b.create_block();
                let check_blk = b.create_block();
                let done_blk = b.create_block();
                let n = b.ins().iconst(I64, TNULL);
                let is_null = b.ins().icmp(IntCC::Equal, v, n);
                b.ins().brif(is_null, null_blk, &[], check_blk, &[]);
                b.switch_to_block(null_blk);
                b.seal_block(null_blk);
                b.def_var(resv, v);
                b.ins().jump(done_blk, &[]);
                b.switch_to_block(check_blk);
                b.seal_block(check_blk);
                let t = self.rcall(b, "plix_truthy", 1, &[v])?;
                let tb = self.tag_bool(b, t);
                b.def_var(resv, tb);
                b.ins().jump(done_blk, &[]);
                b.switch_to_block(done_blk);
                b.seal_block(done_blk);
                return Ok(b.use_var(resv));
            }
            let t = self.rcall(b, "plix_truthy", 1, &[v])?;
            return Ok(self.tag_bool(b, t));
        }
        Ok(v)
    }

    fn store_boxed_into_raw(
        &mut self,
        b: &mut FunctionBuilder,
        loc: Loc,
        v: CVal,
        err_blk: Block,
        what: &str,
    ) -> CResult<()> {
        match loc {
            Loc::RawInt(var) => {
                let raw = self.unbox_int_guard(b, v, err_blk, what)?;
                b.def_var(var, raw);
                Ok(())
            }
            Loc::RawFloat(var) => {
                let raw = self.unbox_float_guard(b, v, err_blk, what)?;
                b.def_var(var, raw);
                Ok(())
            }
            Loc::RawBool(var) => {
                let t = self.rcall(b, "plix_truthy", 1, &[v])?;
                b.def_var(var, t);
                Ok(())
            }
            _ => Err("internal: store_boxed_into_raw on non-raw local".into()),
        }
    }

    /// first store into a pre-declared local (declaration semantics);
    /// allocates a heap Cell for captured variables
    fn declare_local(
        &mut self,
        b: &mut FunctionBuilder,
        fe: &mut FEnv,
        name: &str,
        val: CVal,
    ) -> CResult<()> {
        let loc = *fe
            .vars
            .get(name)
            .ok_or_else(|| format!("internal: local {} not predeclared", name))?;
        match loc {
            Loc::Local(v) => self.assign_loc(b, fe, Loc::Local(v), val),
            Loc::Cell(cv) => {
                let cellv = self.rcall(b, "plix_cell_new", 1, &[val])?;
                let old = b.use_var(cv);
                self.rcall(b, "plix_retain", 1, &[cellv])?;
                self.rcall(b, "plix_release", 1, &[old])?;
                b.def_var(cv, cellv);
                Ok(())
            }
            _ => Err(format!("internal: {} is not a local", name)),
        }
    }

    /// after a terminator (return/break/continue) route further emission
    /// into a sealed, unreachable block
    fn dead_end(&self, b: &mut FunctionBuilder) {
        let dead = b.create_block();
        b.switch_to_block(dead);
        b.seal_block(dead);
    }

    // ---------- statements ----------
    fn stmt(
        &mut self,
        b: &mut FunctionBuilder,
        fe: &mut FEnv,
        s: &Stmt,
        epilogue: Block,
        err_blk: Block,
    ) -> CResult<()> {
        self.stmt_d(b, fe, s, epilogue, err_blk, 0)
    }

    fn stmt_d(
        &mut self,
        b: &mut FunctionBuilder,
        fe: &mut FEnv,
        s: &Stmt,
        epilogue: Block,
        err_blk: Block,
        depth: usize,
    ) -> CResult<()> {
        match &s.node {
            StmtKind::Var {
                kind: _,
                name,
                value,
                ..
            } => {
                // raw path: provable-typed locals skip boxing entirely
                if !(self.is_unit && depth == 0) {
                    if let Some(loc) = fe.vars.get(name).copied() {
                        if let Some(want) = Self::raw_want_of(loc) {
                            let raw =
                                self.emit_typed(b, fe, value, want, err_blk, epilogue, name)?;
                            match loc {
                                Loc::RawInt(v) | Loc::RawFloat(v) | Loc::RawBool(v) => {
                                    b.def_var(v, raw);
                                }
                                _ => unreachable!(),
                            }
                            return Ok(());
                        }
                    }
                }
                let v = self.expr(b, fe, value, epilogue, err_blk)?;
                self.guard(b, err_blk)?;
                // annotated slot stored boxed (global / captured / unit):
                // same boundary guard + canonical representation as interp
                let v = self.guard_boxed(b, s.flags.get(), v, err_blk, name)?;
                if self.is_unit && depth == 0 {
                    let g = *self
                        .res
                        .globals
                        .get(name)
                        .ok_or_else(|| format!("internal: global {} missing", name))?;
                    let idx = b.ins().iconst(I64, g as i64);
                    self.rcall(b, "plix_global_set", 2, &[idx, v])?;
                } else {
                    self.declare_local(b, fe, name, v)?;
                }
                Ok(())
            }
            StmtKind::Func(def) => {
                let clos = self.make_closure(b, fe, def)?;
                self.guard(b, err_blk)?;
                if self.is_unit && depth == 0 {
                    let g = *self
                        .res
                        .globals
                        .get(&def.name)
                        .ok_or_else(|| format!("internal: global {} missing", def.name))?;
                    let idx = b.ins().iconst(I64, g as i64);
                    self.rcall(b, "plix_global_set", 2, &[idx, clos])?;
                } else {
                    self.declare_local(b, fe, &def.name, clos)?;
                }
                Ok(())
            }
            StmtKind::Struct { name, fields } => {
                // create the type object and bind it (global at unit level)
                let (np, nl) = self.str_ptr(b, name)?;
                let defv = self.rcall(b, "plix_struct_new", 2, &[np, nl])?;
                for f in fields {
                    let (kp, kl) = self.str_ptr(b, &f.name)?;
                    let tyn =
                        f.ty.as_ref()
                            .map(|t| match t.name.as_str() {
                                "Option" | "option" => String::new(),
                                other => other.to_string(),
                            })
                            .unwrap_or_default();
                    let (tp, tl) = self.str_ptr(b, &tyn)?;
                    let (dflt, has) = match &f.default {
                        Some(de) => {
                            let v = self.expr(b, fe, de, epilogue, err_blk)?;
                            self.guard(b, err_blk)?;
                            (v, b.ins().iconst(I64, 1))
                        }
                        None => (b.ins().iconst(I64, TNULL), b.ins().iconst(I64, 0)),
                    };
                    self.rcall(
                        b,
                        "plix_struct_field",
                        7,
                        &[defv, kp, kl, tp, tl, dflt, has],
                    )?;
                    self.guard(b, err_blk)?;
                }
                if self.is_unit && depth == 0 {
                    let g = *self
                        .res
                        .globals
                        .get(name)
                        .ok_or_else(|| format!("internal: struct global {} missing", name))?;
                    let idx = b.ins().iconst(I64, g as i64);
                    self.rcall(b, "plix_global_set", 2, &[idx, defv])?;
                } else {
                    self.declare_local(b, fe, name, defv)?;
                }
                Ok(())
            }
            StmtKind::Enum { name: _, variants } => {
                for vdef in variants {
                    if !vdef.fields.is_empty() {
                        // Payload enum constructors are currently provided by
                        // built-in Result/Option. User payload enums are parsed
                        // and checked, but native runtime lowering is deferred.
                        continue;
                    }
                    let (vp, vl) = self.str_ptr(b, &vdef.name)?;
                    let vv = self.rcall(b, "plix_variant_new", 2, &[vp, vl])?;
                    if self.is_unit && depth == 0 {
                        if let Some(&g) = self.res.globals.get(&vdef.name) {
                            let idx = b.ins().iconst(I64, g as i64);
                            self.rcall(b, "plix_global_set", 2, &[idx, vv])?;
                        }
                    }
                }
                Ok(())
            }
            StmtKind::Impl {
                target,
                trait_name,
                methods,
            } => {
                let loc = self.lookup(fe, target, s.span.line)?;
                let defv = self.load_loc(b, fe, loc)?;
                self.guard(b, err_blk)?;
                match trait_name {
                    None => {
                        for m in methods {
                            let fv = self.make_closure(b, fe, m)?;
                            self.guard(b, err_blk)?;
                            let (kp, kl) = self.str_ptr(b, &m.name)?;
                            let zero = b.ins().iconst(I64, 0);
                            self.rcall(
                                b,
                                "plix_struct_method",
                                6,
                                &[defv, kp, kl, fv, zero, zero],
                            )?;
                            self.guard(b, err_blk)?;
                        }
                    }
                    Some(tn) => {
                        // checker-resolved method set (overrides + defaults)
                        let resolved = self
                            .tinfo
                            .structs
                            .get(target)
                            .and_then(|sm| sm.trait_impls.get(tn))
                            .cloned()
                            .unwrap_or_default();
                        let (tp, tl) = self.str_ptr(b, tn)?;
                        for (mname, mdef) in &resolved {
                            let fv = self.make_closure(b, fe, mdef)?;
                            self.guard(b, err_blk)?;
                            let (kp, kl) = self.str_ptr(b, mname)?;
                            self.rcall(b, "plix_struct_method", 6, &[defv, kp, kl, fv, tp, tl])?;
                            self.guard(b, err_blk)?;
                        }
                    }
                }
                Ok(())
            }
            StmtKind::Trait { .. } => Ok(()), // compile-time only
            StmtKind::Import {
                module,
                alias,
                python,
            } => self.emit_import(b, fe, module, alias, *python, s.span.line, err_blk, depth),
            StmtKind::ExprStmt(e) => {
                // statement temporaries die here
                let cp = self.rcall(b, "plix_arena_checkpoint", 0, &[])?;
                b.def_var(fe.cp_var, cp);
                self.expr(b, fe, e, epilogue, err_blk)?;
                self.guard(b, err_blk)?;
                let cpv = b.use_var(fe.cp_var);
                self.rcall(b, "plix_arena_rewind", 1, &[cpv])?;
                Ok(())
            }
            StmtKind::Block(stmts) => {
                for st in stmts {
                    self.stmt_d(b, fe, st, epilogue, err_blk, depth + 1)?;
                }
                Ok(())
            }
            StmtKind::If { cond, then, els } => {
                let t = self.emit_cond(b, fe, cond, epilogue, err_blk)?;
                let then_blk = b.create_block();
                let else_blk = b.create_block();
                let merge_blk = b.create_block();
                b.ins().brif(t, then_blk, &[], else_blk, &[]);
                b.switch_to_block(then_blk);
                b.seal_block(then_blk);
                self.stmt_d(b, fe, then, epilogue, err_blk, depth)?;
                b.ins().jump(merge_blk, &[]);
                b.switch_to_block(else_blk);
                b.seal_block(else_blk);
                if let Some(e) = els {
                    self.stmt_d(b, fe, e, epilogue, err_blk, depth)?;
                }
                b.ins().jump(merge_blk, &[]);
                b.switch_to_block(merge_blk);
                b.seal_block(merge_blk);
                Ok(())
            }
            StmtKind::While { cond, body } => {
                let lcp = b.declare_var(I64);
                let header = b.create_block();
                let body_blk = b.create_block();
                let exit_blk = b.create_block();
                b.ins().jump(header, &[]);
                b.switch_to_block(header);
                let cp = self.rcall(b, "plix_arena_checkpoint", 0, &[])?;
                b.def_var(lcp, cp);
                let t = self.emit_cond(b, fe, cond, epilogue, err_blk)?;
                b.ins().brif(t, body_blk, &[], exit_blk, &[]);
                b.switch_to_block(body_blk);
                b.seal_block(body_blk);
                fe.loop_stack.push(LoopCtx {
                    cont: header,
                    brk: exit_blk,
                    cp: lcp,
                });
                self.stmt_d(b, fe, body, epilogue, err_blk, depth + 1)?;
                fe.loop_stack.pop();
                let cpv = b.use_var(lcp);
                self.rcall(b, "plix_arena_rewind", 1, &[cpv])?;
                b.ins().jump(header, &[]);
                b.seal_block(header);
                b.switch_to_block(exit_blk);
                b.seal_block(exit_blk);
                Ok(())
            }
            StmtKind::ForC {
                init,
                cond,
                step,
                body,
            } => {
                if let Some(i) = init {
                    self.stmt_d(b, fe, i, epilogue, err_blk, depth + 1)?;
                }
                let lcp = b.declare_var(I64);
                let header = b.create_block();
                let body_blk = b.create_block();
                let step_blk = b.create_block();
                let exit_blk = b.create_block();
                b.ins().jump(header, &[]);
                b.switch_to_block(header);
                let cp = self.rcall(b, "plix_arena_checkpoint", 0, &[])?;
                b.def_var(lcp, cp);
                if let Some(c) = cond {
                    let t = self.emit_cond(b, fe, c, epilogue, err_blk)?;
                    b.ins().brif(t, body_blk, &[], exit_blk, &[]);
                } else {
                    b.ins().jump(body_blk, &[]);
                }
                b.switch_to_block(body_blk);
                b.seal_block(body_blk);
                fe.loop_stack.push(LoopCtx {
                    cont: step_blk,
                    brk: exit_blk,
                    cp: lcp,
                });
                self.stmt_d(b, fe, body, epilogue, err_blk, depth + 1)?;
                fe.loop_stack.pop();
                b.ins().jump(step_blk, &[]);
                b.switch_to_block(step_blk);
                let cpv = b.use_var(lcp);
                self.rcall(b, "plix_arena_rewind", 1, &[cpv])?;
                if let Some(st) = step {
                    self.expr(b, fe, st, epilogue, err_blk)?;
                    self.guard(b, err_blk)?;
                }
                b.ins().jump(header, &[]);
                b.seal_block(step_blk);
                b.seal_block(header);
                b.switch_to_block(exit_blk);
                b.seal_block(exit_blk);
                Ok(())
            }
            StmtKind::ForIn {
                name, iter, body, ..
            } => {
                let it = self.expr(b, fe, iter, epilogue, err_blk)?;
                self.guard(b, err_blk)?;
                let arr = self.rcall(b, "plix_forin_iter", 1, &[it])?;
                self.guard(b, err_blk)?;
                // owned temporaries: iteration container + counter
                let arr_v = b.declare_var(I64);
                let zero1 = b.ins().iconst(I64, TNULL);
                b.def_var(arr_v, zero1);
                fe.locals_all.push(arr_v);
                self.rcall(b, "plix_retain", 1, &[arr])?;
                b.def_var(arr_v, arr);
                let i_v = b.declare_var(I64);
                let zero = b.ins().iconst(I64, tconst_int(0));
                b.def_var(i_v, zero);
                fe.locals_all.push(i_v);
                let loop_loc = *fe
                    .vars
                    .get(name.as_str())
                    .ok_or_else(|| format!("internal: for-in local {} missing", name))?;

                let lcp = b.declare_var(I64);
                let header = b.create_block();
                let body_blk = b.create_block();
                let inc_blk = b.create_block();
                let exit_blk = b.create_block();
                b.ins().jump(header, &[]);
                b.switch_to_block(header);
                let cp = self.rcall(b, "plix_arena_checkpoint", 0, &[])?;
                b.def_var(lcp, cp);
                let cur_arr = b.use_var(arr_v);
                let len_v = self.rcall(b, "plix_len", 1, &[cur_arr])?;
                self.guard(b, err_blk)?;
                let cur_i = b.use_var(i_v);
                let cond_less = b.ins().icmp(IntCC::SignedLessThan, cur_i, len_v);
                b.ins().brif(cond_less, body_blk, &[], exit_blk, &[]);
                b.switch_to_block(body_blk);
                b.seal_block(body_blk);
                let arr2 = b.use_var(arr_v);
                let i2 = b.use_var(i_v);
                let elem = self.rcall(b, "plix_index", 2, &[arr2, i2])?;
                self.guard(b, err_blk)?;
                if Self::raw_want_of(loop_loc).is_some() {
                    self.store_boxed_into_raw(b, loop_loc, elem, err_blk, "for-in element")?;
                } else {
                    // annotated loop variable kept boxed: guard at the boundary
                    let elem =
                        self.guard_boxed(b, s.flags.get(), elem, err_blk, "for-in element")?;
                    self.assign_loc(b, fe, loop_loc, elem)?;
                }
                fe.loop_stack.push(LoopCtx {
                    cont: inc_blk,
                    brk: exit_blk,
                    cp: lcp,
                });
                self.stmt_d(b, fe, body, epilogue, err_blk, depth + 1)?;
                fe.loop_stack.pop();
                b.ins().jump(inc_blk, &[]);
                b.switch_to_block(inc_blk);
                let cpv = b.use_var(lcp);
                self.rcall(b, "plix_arena_rewind", 1, &[cpv])?;
                // i += 1 (tagged: +2)
                let i3 = b.use_var(i_v);
                let two = b.ins().iconst(I64, 2);
                let inext = b.ins().iadd(i3, two);
                b.def_var(i_v, inext);
                b.ins().jump(header, &[]);
                b.seal_block(inc_blk);
                b.seal_block(header);
                b.switch_to_block(exit_blk);
                b.seal_block(exit_blk);
                Ok(())
            }
            StmtKind::MatchStmt { subject, arms } => {
                let _v = self.match_common(b, fe, subject, arms, true, epilogue, err_blk)?;
                Ok(())
            }
            StmtKind::Return(v) => {
                let val = match v {
                    Some(e) => {
                        if self.static_ty(fe, e) != STy::Box {
                            let rv = self
                                .emit_raw(b, fe, e, epilogue, err_blk)?
                                .ok_or("internal: raw return emission")?;
                            self.box_raw(b, rv)?
                        } else {
                            let x = self.expr(b, fe, e, epilogue, err_blk)?;
                            self.guard(b, err_blk)?;
                            // declared return type on a dynamic value: enforce
                            // at the return site (interpreter parity), with
                            // the canonical representation (float widening /
                            // truthiness for bool), including nullable scalar
                            // forms such as `int?`.
                            if fe.ret_flags != 0 {
                                let what = format!("return value of {}", fe.fn_name);
                                self.guard_boxed(b, fe.ret_flags, x, err_blk, &what)?
                            } else {
                                x
                            }
                        }
                    }
                    None => b.ins().iconst(I64, TNULL),
                };
                let val = if fe.ret_flags != 0 {
                    let what = format!("return value of {}", fe.fn_name);
                    self.guard_boxed(b, fe.ret_flags, val, err_blk, &what)?
                } else {
                    val
                };
                // own the return value; frame_pop adopts it
                self.rcall(b, "plix_retain", 1, &[val])?;
                b.def_var(fe.ret_var, val);
                b.ins().jump(epilogue, &[]);
                self.dead_end(b);
                Ok(())
            }
            StmtKind::Break => {
                let Some(lc) = fe.loop_stack.last() else {
                    return Err("break outside of loop".into());
                };
                let brk = lc.brk;
                b.ins().jump(brk, &[]);
                self.dead_end(b);
                Ok(())
            }
            StmtKind::Continue => {
                let Some(lc) = fe.loop_stack.last() else {
                    return Err("continue outside of loop".into());
                };
                let cont = lc.cont;
                let cp = lc.cp;
                let cpv = b.use_var(cp);
                self.rcall(b, "plix_arena_rewind", 1, &[cpv])?;
                b.ins().jump(cont, &[]);
                self.dead_end(b);
                Ok(())
            }
        }
    }

    // match, shared by the statement and expression forms
    fn match_common(
        &mut self,
        b: &mut FunctionBuilder,
        fe: &mut FEnv,
        subject: &Expr,
        arms: &[MatchArm],
        is_stmt: bool,
        epilogue: Block,
        err_blk: Block,
    ) -> CResult<Option<CVal>> {
        let sv = self.expr(b, fe, subject, epilogue, err_blk)?;
        self.guard(b, err_blk)?;
        // subject held in an owned variable for the whole dispatch
        let subj_v = b.declare_var(I64);
        let n0 = b.ins().iconst(I64, TNULL);
        b.def_var(subj_v, n0);
        fe.locals_all.push(subj_v);
        self.rcall(b, "plix_retain", 1, &[sv])?;
        b.def_var(subj_v, sv);

        // expression form: result variable (arena-managed values, no extra retain)
        let res_v = if is_stmt {
            None
        } else {
            let rv = b.declare_var(I64);
            let nullv = b.ins().iconst(I64, TNULL);
            b.def_var(rv, nullv);
            Some(rv)
        };

        let end_blk = b.create_block();
        let mut last_fell_through = true;
        for arm in arms {
            // invariant: the current block is where this arm's tests start
            let arm_blk = b.create_block();
            let next_arm_blk = b.create_block();
            let mut falls_to_next = true;
            for pat in &arm.pats {
                match pat {
                    Pattern::Wildcard => {
                        b.ins().jump(arm_blk, &[]);
                        falls_to_next = false;
                        break;
                    }
                    Pattern::Ident(n) => {
                        let cur = b.use_var(subj_v);
                        let used = self.rcall(b, "plix_var_use", 1, &[cur])?;
                        self.bind_match_name(b, fe, n, used, err_blk)?;
                        b.ins().jump(arm_blk, &[]);
                        falls_to_next = false;
                        break;
                    }
                    Pattern::Variant(name, args) => {
                        let subj = b.use_var(subj_v);
                        let (np, nl) = self.str_ptr(b, name)?;
                        let ok = self.rcall(b, "plix_variant_is", 3, &[subj, np, nl])?;
                        let bind_blk = b.create_block();
                        let cont = b.create_block();
                        b.ins().brif(ok, bind_blk, &[], cont, &[]);
                        b.switch_to_block(bind_blk);
                        b.seal_block(bind_blk);
                        for (i, ap) in args.iter().enumerate() {
                            if let Pattern::Ident(n) = ap {
                                let idx = b.ins().iconst(I64, i as i64);
                                let fv0 = self.rcall(b, "plix_variant_field", 2, &[subj, idx])?;
                                let fv = self.rcall(b, "plix_var_use", 1, &[fv0])?;
                                self.bind_match_name(b, fe, n, fv, err_blk)?;
                            }
                        }
                        b.ins().jump(arm_blk, &[]);
                        b.switch_to_block(cont);
                        b.seal_block(cont);
                    }
                    _ => {
                        let pv = self.pattern_value(b, pat)?;
                        let subj = b.use_var(subj_v);
                        let eq = self.rcall(b, "plix_eq", 2, &[subj, pv])?;
                        self.guard(b, err_blk)?;
                        let cont = b.create_block();
                        b.ins().brif(eq, arm_blk, &[], cont, &[]);
                        b.switch_to_block(cont);
                        b.seal_block(cont);
                    }
                }
            }
            if falls_to_next {
                b.ins().jump(next_arm_blk, &[]);
            }
            // arm body
            b.switch_to_block(arm_blk);
            b.seal_block(arm_blk);
            match &arm.body {
                MatchBody::Expr(e) => {
                    let v = self.expr(b, fe, e, epilogue, err_blk)?;
                    self.guard(b, err_blk)?;
                    if let Some(rv) = res_v {
                        b.def_var(rv, v);
                    }
                }
                MatchBody::Block(stmts) => {
                    for st in stmts {
                        self.stmt_d(b, fe, st, epilogue, err_blk, 1)?;
                    }
                }
            }
            b.ins().jump(end_blk, &[]);
            // next arm's tests run in next_arm_blk; if this arm was
            // irrefutable that block is dead but must still absorb emission
            b.switch_to_block(next_arm_blk);
            if !falls_to_next {
                b.seal_block(next_arm_blk);
            }
            last_fell_through = falls_to_next;
        }
        // no arm matched -> runtime error
        let (p, l) = self.str_ptr(b, "non-exhaustive match: no arm matched")?;
        self.rcall(b, "plix_set_error", 2, &[p, l])?;
        b.ins().jump(err_blk, &[]);
        if last_fell_through {
            // the fallthrough test block has real predecessors: seal it now
            b.seal_block(b.current_block().unwrap());
        }
        b.switch_to_block(end_blk);
        b.seal_block(end_blk);
        Ok(res_v.map(|rv| b.use_var(rv)))
    }

    fn bind_match_name(
        &mut self,
        b: &mut FunctionBuilder,
        fe: &mut FEnv,
        n: &str,
        used: CVal,
        err_blk: Block,
    ) -> CResult<()> {
        match fe.vars.get(n).copied() {
            Some(loc) if Self::raw_want_of(loc).is_some() => {
                self.store_boxed_into_raw(b, loc, used, err_blk, "match binding")?;
                self.rcall(b, "plix_release", 1, &[used])?;
            }
            _ => self.declare_local(b, fe, n, used)?,
        }
        Ok(())
    }

    fn pattern_value(&mut self, b: &mut FunctionBuilder, pat: &Pattern) -> CResult<CVal> {
        Ok(match pat {
            Pattern::Null => b.ins().iconst(I64, TNULL),
            Pattern::Bool(x) => b.ins().iconst(I64, if *x { TTRUE } else { TFALSE }),
            Pattern::Int(i) => b
                .ins()
                .iconst(I64, tconst_int((*i).clamp(INT_MIN, INT_MAX))),
            Pattern::Float(f) => {
                let bits = b.ins().iconst(I64, f.to_bits() as i64);
                self.rcall(b, "plix_float_bits", 1, &[bits])?
            }
            Pattern::Str(s) => {
                let (p, l) = self.str_ptr(b, s)?;
                self.rcall(b, "plix_str_new", 2, &[p, l])?
            }
            Pattern::Ident(_) | Pattern::Variant(_, _) | Pattern::Wildcard => {
                b.ins().iconst(I64, TTRUE)
            }
        })
    }

    fn emit_import(
        &mut self,
        b: &mut FunctionBuilder,
        fe: &mut FEnv,
        module: &str,
        alias: &str,
        python: bool,
        line: u32,
        err_blk: Block,
        depth: usize,
    ) -> CResult<()> {
        let val: CVal;
        if python {
            // val = py.import(module)
            let pymod = self.lookup(fe, "py", line)?;
            let mv = self.load_loc(b, fe, pymod)?;
            let (np, nl) = self.str_ptr(b, "import")?;
            let imp = self.rcall(b, "plix_member", 3, &[mv, np, nl])?;
            self.guard(b, err_blk)?;
            let (sp, sl) = self.str_ptr(b, module)?;
            let namev = self.rcall(b, "plix_str_new", 2, &[sp, sl])?;
            let addr = self.stack_args(b, &[namev])?;
            let one = b.ins().iconst(I64, 1);
            val = self.rcall(b, "plix_call", 3, &[imp, addr, one])?;
            self.guard(b, err_blk)?;
        } else if module.ends_with(".px") {
            let meta =
                self.mods.iter().find(|m| m.alias == alias).ok_or_else(|| {
                    format!("native: module \"{}\" unknown (line {})", alias, line)
                })?;
            let nullv = b.ins().iconst(I64, TNULL);
            let zero = b.ins().iconst(I64, 0);
            let fr = self
                .shared
                .module
                .declare_func_in_func(meta.init_fid, b.func);
            b.ins().call(fr, &[nullv, nullv, zero]);
            self.guard(b, err_blk)?;
            // build the module's export map from its globals
            let m = self.rcall(b, "plix_map_new", 0, &[])?;
            for (name, gidx) in &meta.exports {
                let idxc = b.ins().iconst(I64, *gidx as i64);
                let gv = self.rcall(b, "plix_global_get", 1, &[idxc])?;
                let (kp, kl) = self.str_ptr(b, name)?;
                self.rcall(b, "plix_map_set", 4, &[m, kp, kl, gv])?;
                self.guard(b, err_blk)?;
            }
            val = m;
        } else {
            // native stdlib module: `import "fs";` (optionally `as f`)
            if alias == module && self.res.globals.contains_key(alias) {
                // the builtin module map is already a global under this name
                return Ok(());
            }
            let loc = self.lookup(fe, module, line)?;
            val = self.load_loc(b, fe, loc)?;
        }
        if self.is_unit && depth == 0 {
            if let Some(&g) = self.res.globals.get(alias) {
                let idx = b.ins().iconst(I64, g as i64);
                self.rcall(b, "plix_global_set", 2, &[idx, val])?;
                return Ok(());
            }
        }
        self.declare_local(b, fe, alias, val)
    }

    // ---------- closures ----------
    fn make_closure(
        &mut self,
        b: &mut FunctionBuilder,
        fe: &mut FEnv,
        def: &Rc<FuncDef>,
    ) -> CResult<CVal> {
        let id = FuncDef::id(def);
        let fid = *self
            .fns
            .get(&id)
            .ok_or_else(|| format!("internal: unknown fn {}", def.name))?;
        let ores = self
            .res
            .fns
            .get(&id)
            .ok_or_else(|| format!("internal: no resolution for {}", def.name))?;
        let mut cells: Vec<CVal> = Vec::new();
        for name in &ores.captures {
            let loc = self.lookup(fe, name, def.span.line)?;
            match loc {
                Loc::Cell(cv) => cells.push(b.use_var(cv)),
                Loc::Free(i) => {
                    let cell = b
                        .ins()
                        .load(I64, MemFlags::trusted(), fe.cells_ptr, (i * 8) as i32);
                    cells.push(cell);
                }
                _ => {
                    return Err(format!(
                        "internal: capture \"{}\" of {} is not a cell",
                        name, def.name
                    ));
                }
            }
        }
        let fr = self.shared.module.declare_func_in_func(fid, b.func);
        let code = b.ins().func_addr(I64, fr);
        let cells_addr = self.stack_args(b, &cells)?;
        let ncells = b.ins().iconst(I64, cells.len() as i64);
        let (np, nl) = self.str_ptr(b, &def.name)?;
        self.rcall(
            b,
            "plix_closure_new",
            5,
            &[code, cells_addr, ncells, np, nl],
        )
    }

    // ---------- expressions ----------
    fn expr(
        &mut self,
        b: &mut FunctionBuilder,
        fe: &mut FEnv,
        e: &Expr,
        epilogue: Block,
        err_blk: Block,
    ) -> CResult<CVal> {
        match &e.node {
            ExprKind::Null => Ok(b.ins().iconst(I64, TNULL)),
            ExprKind::Bool(x) => Ok(b.ins().iconst(I64, if *x { TTRUE } else { TFALSE })),
            ExprKind::Int(i) => {
                if *i >= INT_MIN && *i <= INT_MAX {
                    Ok(b.ins().iconst(I64, tconst_int(*i)))
                } else {
                    // out of boxed range: becomes a float like the interpreter
                    let bits = b.ins().iconst(I64, (*i as f64).to_bits() as i64);
                    self.rcall(b, "plix_float_bits", 1, &[bits])
                }
            }
            ExprKind::Float(f) => {
                let bits = b.ins().iconst(I64, f.to_bits() as i64);
                self.rcall(b, "plix_float_bits", 1, &[bits])
            }
            ExprKind::Str(s) => {
                let (p, l) = self.str_ptr(b, s)?;
                self.rcall(b, "plix_str_new", 2, &[p, l])
            }
            ExprKind::Ident(name) => {
                let loc = self.lookup(fe, name, e.span.line)?;
                self.load_loc(b, fe, loc)
            }
            ExprKind::Array(items) => {
                let mut vals = Vec::with_capacity(items.len());
                for it in items {
                    let v = self.expr(b, fe, it, epilogue, err_blk)?;
                    self.guard(b, err_blk)?;
                    vals.push(v);
                }
                let addr = self.stack_args(b, &vals)?;
                let n = b.ins().iconst(I64, vals.len() as i64);
                self.rcall(b, "plix_array_new", 2, &[addr, n])
            }
            ExprKind::Object(props) => {
                let m = self.rcall(b, "plix_map_new", 0, &[])?;
                for (k, ve) in props {
                    let v = self.expr(b, fe, ve, epilogue, err_blk)?;
                    self.guard(b, err_blk)?;
                    let (kp, kl) = self.str_ptr(b, k)?;
                    self.rcall(b, "plix_map_set", 4, &[m, kp, kl, v])?;
                    self.guard(b, err_blk)?;
                }
                Ok(m)
            }
            ExprKind::Unary(op, x) => {
                let v = self.expr(b, fe, x, epilogue, err_blk)?;
                self.guard(b, err_blk)?;
                match op {
                    UnOp::Neg => self.rcall(b, "plix_neg", 1, &[v]),
                    UnOp::Not => self.rcall(b, "plix_not", 1, &[v]),
                    UnOp::BitNot => self.rcall(b, "plix_bitnot", 1, &[v]),
                }
            }
            ExprKind::Borrow { expr, .. } => self.expr(b, fe, expr, epilogue, err_blk),
            ExprKind::Binary(op, x, y) => self.binary(b, fe, *op, x, y, epilogue, err_blk),
            ExprKind::Logical(op, x, y) => {
                let lhs = self.expr(b, fe, x, epilogue, err_blk)?;
                self.guard(b, err_blk)?;
                let t = self.rcall(b, "plix_truthy", 1, &[lhs])?;
                let rhs_blk = b.create_block();
                let done_blk = b.create_block();
                match op {
                    LogicalOp::And => {
                        b.ins().brif(t, rhs_blk, &[], done_blk, &[]);
                    }
                    LogicalOp::Or => {
                        b.ins().brif(t, done_blk, &[], rhs_blk, &[]);
                    }
                }
                let outv = b.declare_var(I64);
                b.def_var(outv, lhs);
                b.switch_to_block(rhs_blk);
                b.seal_block(rhs_blk);
                let rhs = self.expr(b, fe, y, epilogue, err_blk)?;
                self.guard(b, err_blk)?;
                b.def_var(outv, rhs);
                b.ins().jump(done_blk, &[]);
                b.switch_to_block(done_blk);
                b.seal_block(done_blk);
                Ok(b.use_var(outv))
            }
            ExprKind::Ternary(c, x, y) => {
                let cv = self.expr(b, fe, c, epilogue, err_blk)?;
                self.guard(b, err_blk)?;
                let t = self.rcall(b, "plix_truthy", 1, &[cv])?;
                let x_blk = b.create_block();
                let y_blk = b.create_block();
                let done_blk = b.create_block();
                b.ins().brif(t, x_blk, &[], y_blk, &[]);
                let outv = b.declare_var(I64);
                b.switch_to_block(x_blk);
                b.seal_block(x_blk);
                let xv = self.expr(b, fe, x, epilogue, err_blk)?;
                self.guard(b, err_blk)?;
                b.def_var(outv, xv);
                b.ins().jump(done_blk, &[]);
                b.switch_to_block(y_blk);
                b.seal_block(y_blk);
                let yv = self.expr(b, fe, y, epilogue, err_blk)?;
                self.guard(b, err_blk)?;
                b.def_var(outv, yv);
                b.ins().jump(done_blk, &[]);
                b.switch_to_block(done_blk);
                b.seal_block(done_blk);
                Ok(b.use_var(outv))
            }
            ExprKind::Assign { target, op, value } => self.assign_expr(
                b,
                fe,
                target,
                *op,
                value,
                epilogue,
                err_blk,
                e.span.line,
                e.flags.get(),
            ),
            ExprKind::Call(callee, args) => {
                // direct-call shortcut: named top-level function, never
                // reassigned, with no captures -> plain native call
                let mut direct: Option<usize> = None;
                if let ExprKind::Ident(n) = &callee.node {
                    if !self.assigned.contains(n) {
                        if let Some(id) = self.find_stable_fn(n) {
                            direct = Some(id);
                        }
                    }
                }
                let mut vals = Vec::with_capacity(args.len());
                for a in args {
                    let v = self.expr(b, fe, a, epilogue, err_blk)?;
                    self.guard(b, err_blk)?;
                    vals.push(v);
                }
                let addr = self.stack_args(b, &vals)?;
                let n = b.ins().iconst(I64, vals.len() as i64);
                match direct {
                    Some(id) => {
                        let fidr = self.fns[&id];
                        let nullv = b.ins().iconst(I64, TNULL);
                        let fr = self.shared.module.declare_func_in_func(fidr, b.func);
                        let call = b.ins().call(fr, &[nullv, addr, n]);
                        Ok(b.inst_results(call)[0])
                    }
                    None => {
                        let f = self.expr(b, fe, callee, epilogue, err_blk)?;
                        self.guard(b, err_blk)?;
                        self.rcall(b, "plix_call", 3, &[f, addr, n])
                    }
                }
            }
            ExprKind::Index(obj, idx) => {
                let o = self.expr(b, fe, obj, epilogue, err_blk)?;
                self.guard(b, err_blk)?;
                let i = self.expr(b, fe, idx, epilogue, err_blk)?;
                self.guard(b, err_blk)?;
                self.rcall(b, "plix_index", 2, &[o, i])
            }
            ExprKind::Slice { obj, start, end } => {
                let o = self.expr(b, fe, obj, epilogue, err_blk)?;
                self.guard(b, err_blk)?;
                let (sv, hs) = match start {
                    Some(sx) => {
                        let v = self.expr(b, fe, sx, epilogue, err_blk)?;
                        self.guard(b, err_blk)?;
                        (self.untag(b, v), b.ins().iconst(I64, 1))
                    }
                    None => (b.ins().iconst(I64, 0), b.ins().iconst(I64, 0)),
                };
                let (ev, he) = match end {
                    Some(ex) => {
                        let v = self.expr(b, fe, ex, epilogue, err_blk)?;
                        self.guard(b, err_blk)?;
                        (self.untag(b, v), b.ins().iconst(I64, 1))
                    }
                    None => (b.ins().iconst(I64, 0), b.ins().iconst(I64, 0)),
                };
                self.rcall(b, "plix_slice", 5, &[o, sv, hs, ev, he])
            }
            ExprKind::Member(obj, name) => {
                let o = self.expr(b, fe, obj, epilogue, err_blk)?;
                self.guard(b, err_blk)?;
                let (np, nl) = self.str_ptr(b, name)?;
                self.rcall(b, "plix_member", 3, &[o, np, nl])
            }
            ExprKind::StructLit { name, fields } => {
                let loc = self.lookup(fe, name, e.span.line)?;
                let defv = self.load_loc(b, fe, loc)?;
                self.guard(b, err_blk)?;
                let mut vals = Vec::with_capacity(fields.len());
                let mut names = String::new();
                for (fname, ve) in fields {
                    if !names.is_empty() {
                        names.push('\0');
                    }
                    names.push_str(fname);
                    let v = self.expr(b, fe, ve, epilogue, err_blk)?;
                    self.guard(b, err_blk)?;
                    vals.push(v);
                }
                let addr = self.stack_args(b, &vals)?;
                let n = b.ins().iconst(I64, vals.len() as i64);
                let (np, nl) = self.str_ptr(b, &names)?;
                self.rcall(b, "plix_instance_new", 5, &[defv, np, nl, addr, n])
            }
            ExprKind::FuncLit(def) => {
                let v = self.make_closure(b, fe, def)?;
                self.guard(b, err_blk)?;
                Ok(v)
            }
            ExprKind::Match { subject, arms } => {
                let v = self.match_common(b, fe, subject, arms, false, epilogue, err_blk)?;
                Ok(v.unwrap())
            }
        }
    }

    /// top-level function with no captures whose global binding is never
    /// reassigned — eligible for a direct native call
    fn find_stable_fn(&self, name: &str) -> Option<usize> {
        for (id, (n, unit)) in self.fn_name_of.iter() {
            if n == name && *unit == 0 {
                if let Some(r) = self.res.fns.get(id) {
                    if r.captures.is_empty() {
                        if matches!(self.res.globals.get(name), Some(_)) {
                            return Some(*id);
                        }
                    }
                }
            }
        }
        None
    }

    fn binary(
        &mut self,
        b: &mut FunctionBuilder,
        fe: &mut FEnv,
        op: BinOp,
        x: &Expr,
        y: &Expr,
        epilogue: Block,
        err_blk: Block,
    ) -> CResult<CVal> {
        let a = self.expr(b, fe, x, epilogue, err_blk)?;
        self.guard(b, err_blk)?;
        let c2 = self.expr(b, fe, y, epilogue, err_blk)?;
        self.guard(b, err_blk)?;
        // int fast path for add/sub and ordered comparisons (range checked)
        let fast = matches!(
            op,
            BinOp::Add | BinOp::Sub | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
        );
        if !fast {
            let r = self.rt_binop(b, op, a, c2)?;
            self.guard(b, err_blk)?;
            return Ok(r);
        }

        let resv = b.declare_var(I64);
        let slow_blk = b.create_block();
        let check_blk = b.create_block();
        let done_blk = b.create_block();

        let one = b.ins().iconst(I64, 1);
        let a_odd = b.ins().band(a, one);
        let b_odd = b.ins().band(c2, one);
        let both = b.ins().band(a_odd, b_odd);
        b.ins().brif(both, check_blk, &[], slow_blk, &[]);
        b.switch_to_block(check_blk);
        b.seal_block(check_blk);

        let is_arith = matches!(op, BinOp::Add | BinOp::Sub);
        if is_arith {
            // tagged add: a+b-1 ; tagged sub: a-b+1
            let onea = b.ins().iconst(I64, 1);
            let oneb = b.ins().iconst(I64, 1);
            let s = match op {
                BinOp::Add => {
                    let t = b.ins().iadd(a, c2);
                    b.ins().isub(t, onea)
                }
                _ => {
                    let t = b.ins().isub(a, c2);
                    b.ins().iadd(t, oneb)
                }
            };
            let lo = b.ins().iconst(I64, TAGGED_MIN);
            let hi = b.ins().iconst(I64, TAGGED_MAX);
            let c_lo = b.ins().icmp(IntCC::SignedGreaterThanOrEqual, s, lo);
            let c_hi = b.ins().icmp(IntCC::SignedLessThanOrEqual, s, hi);
            let in_range = b.ins().band(c_lo, c_hi);
            let ok_blk = b.create_block();
            b.ins().brif(in_range, ok_blk, &[], slow_blk, &[]);
            b.switch_to_block(ok_blk);
            b.seal_block(ok_blk);
            b.def_var(resv, s);
            b.ins().jump(done_blk, &[]);
        } else {
            let fr = match op {
                BinOp::Lt => self.cmp_tagged(b, a, c2, IntCC::SignedLessThan),
                BinOp::Le => self.cmp_tagged(b, a, c2, IntCC::SignedLessThanOrEqual),
                BinOp::Gt => self.cmp_tagged(b, a, c2, IntCC::SignedGreaterThan),
                _ => self.cmp_tagged(b, a, c2, IntCC::SignedGreaterThanOrEqual),
            };
            b.def_var(resv, fr);
            b.ins().jump(done_blk, &[]);
        }

        b.switch_to_block(slow_blk);
        b.seal_block(slow_blk);
        let sv = self.rt_binop(b, op, a, c2)?;
        b.def_var(resv, sv);
        b.ins().jump(done_blk, &[]);
        b.switch_to_block(done_blk);
        b.seal_block(done_blk);
        Ok(b.use_var(resv))
    }

    /// tagged int comparison returning a tagged bool (true=2, false=6)
    fn cmp_tagged(&self, b: &mut FunctionBuilder, a: CVal, c: CVal, cc: IntCC) -> CVal {
        let i8v = b.ins().icmp(cc, a, c);
        let iv = b.ins().uextend(I64, i8v);
        self.tag_bool(b, iv)
    }

    /// raw 0/1 -> tagged bool (false=6, true=2): out = 6 - 4*x
    fn tag_bool(&self, b: &mut FunctionBuilder, i64v: CVal) -> CVal {
        let four = b.ins().iconst(I64, 4);
        let six = b.ins().iconst(I64, 6);
        let fx = b.ins().imul(four, i64v);
        b.ins().isub(six, fx)
    }

    fn rt_binop(&mut self, b: &mut FunctionBuilder, op: BinOp, a: CVal, c: CVal) -> CResult<CVal> {
        match op {
            BinOp::Add => self.rcall(b, "plix_add", 2, &[a, c]),
            BinOp::Sub => self.rcall(b, "plix_sub", 2, &[a, c]),
            BinOp::Mul => self.rcall(b, "plix_mul", 2, &[a, c]),
            BinOp::Div => self.rcall(b, "plix_div", 2, &[a, c]),
            BinOp::Mod => self.rcall(b, "plix_rem", 2, &[a, c]),
            BinOp::Eq => {
                let v = self.rcall(b, "plix_eq", 2, &[a, c])?;
                Ok(self.tag_bool(b, v))
            }
            BinOp::Ne => {
                let v = self.rcall(b, "plix_ne", 2, &[a, c])?;
                Ok(self.tag_bool(b, v))
            }
            BinOp::Lt => {
                let v = self.rcall(b, "plix_lt", 2, &[a, c])?;
                Ok(self.tag_bool(b, v))
            }
            BinOp::Le => {
                let v = self.rcall(b, "plix_le", 2, &[a, c])?;
                Ok(self.tag_bool(b, v))
            }
            BinOp::Gt => {
                let v = self.rcall(b, "plix_gt", 2, &[a, c])?;
                Ok(self.tag_bool(b, v))
            }
            BinOp::Ge => {
                let v = self.rcall(b, "plix_ge", 2, &[a, c])?;
                Ok(self.tag_bool(b, v))
            }
            BinOp::BAnd => self.rcall(b, "plix_band", 2, &[a, c]),
            BinOp::BOr => self.rcall(b, "plix_bor", 2, &[a, c]),
            BinOp::BXor => self.rcall(b, "plix_bxor", 2, &[a, c]),
            BinOp::Shl => self.rcall(b, "plix_shl", 2, &[a, c]),
            BinOp::Shr => self.rcall(b, "plix_shr", 2, &[a, c]),
        }
    }

    fn assign_expr(
        &mut self,
        b: &mut FunctionBuilder,
        fe: &mut FEnv,
        target: &AssignTarget,
        op: AssignOp,
        value: &Expr,
        epilogue: Block,
        err_blk: Block,
        line: u32,
        flags: u8,
    ) -> CResult<CVal> {
        let compound = |op: AssignOp| -> BinOp {
            match op {
                AssignOp::Add => BinOp::Add,
                AssignOp::Sub => BinOp::Sub,
                AssignOp::Mul => BinOp::Mul,
                AssignOp::Div => BinOp::Div,
                AssignOp::Mod => BinOp::Mod,
                AssignOp::Eq => unreachable!(),
            }
        };
        match target {
            AssignTarget::Ident(n) => {
                let loc = self.lookup(fe, n, line)?;
                // raw local: check-assigned, unboxed store
                if let Some(want) = Self::raw_want_of(loc) {
                    let raw = if op == AssignOp::Eq {
                        self.emit_typed(b, fe, value, want, err_blk, epilogue, n)?
                    } else {
                        self.raw_compound(b, fe, loc, want, op, value, epilogue, err_blk)?
                    };
                    match loc {
                        Loc::RawInt(v) | Loc::RawFloat(v) | Loc::RawBool(v) => {
                            b.def_var(v, raw);
                            let rv = match want {
                                Want::Int => RawVal::I(raw),
                                Want::Float => RawVal::F(raw),
                                Want::Bool => RawVal::B(raw),
                            };
                            return self.box_raw(b, rv);
                        }
                        _ => unreachable!(),
                    }
                }
                let rhs = self.expr(b, fe, value, epilogue, err_blk)?;
                self.guard(b, err_blk)?;
                let newv = if op == AssignOp::Eq {
                    // typed slot kept boxed: boundary guard/conversion
                    self.guard_boxed(b, flags, rhs, err_blk, n)?
                } else if flags & (crate::ast::FLAG_GUARD_INT | crate::ast::FLAG_GUARD_FLOAT) != 0 {
                    // compound on a boxed typed slot: guard the rhs exactly
                    // like the interpreter, keep strict-overflow semantics
                    let gi = flags & crate::ast::FLAG_GUARD_INT != 0;
                    let raw_rhs = if gi {
                        self.unbox_int_guard(b, rhs, err_blk, "compound assignment")?
                    } else {
                        self.unbox_float_guard(b, rhs, err_blk, "compound assignment")?
                    };
                    let rhs2 = if gi {
                        self.rcall(b, "plix_int", 1, &[raw_rhs])?
                    } else {
                        self.box_raw(b, RawVal::F(raw_rhs))?
                    };
                    let old = self.load_loc(b, fe, loc)?;
                    if gi && flags & crate::ast::FLAG_STRICT_INT_ARITH != 0 {
                        // typed int arithmetic: checked i64 (no promotion)
                        let raw_old = self.unbox_int_guard(b, old, err_blk, "typed slot")?;
                        let bop = compound(op);
                        let r = self.int_checked(b, bop, raw_old, raw_rhs, err_blk)?;
                        self.rcall(b, "plix_int", 1, &[r])?
                    } else {
                        let r = self.rt_binop(b, compound(op), old, rhs2)?;
                        self.guard(b, err_blk)?;
                        r
                    }
                } else {
                    let old = self.load_loc(b, fe, loc)?;
                    let r = self.rt_binop(b, compound(op), old, rhs)?;
                    self.guard(b, err_blk)?;
                    r
                };
                self.assign_loc(b, fe, loc, newv)?;
                Ok(newv)
            }
            AssignTarget::Index(oe, ie) => {
                let o = self.expr(b, fe, oe, epilogue, err_blk)?;
                self.guard(b, err_blk)?;
                let i = self.expr(b, fe, ie, epilogue, err_blk)?;
                self.guard(b, err_blk)?;
                let rhs = self.expr(b, fe, value, epilogue, err_blk)?;
                self.guard(b, err_blk)?;
                let newv = if op == AssignOp::Eq {
                    rhs
                } else {
                    let old = self.rcall(b, "plix_index", 2, &[o, i])?;
                    self.guard(b, err_blk)?;
                    let r = self.rt_binop(b, compound(op), old, rhs)?;
                    self.guard(b, err_blk)?;
                    r
                };
                let r2 = self.rcall(b, "plix_index_set", 3, &[o, i, newv])?;
                self.guard(b, err_blk)?;
                Ok(r2)
            }
            AssignTarget::Member(oe, name) => {
                let o = self.expr(b, fe, oe, epilogue, err_blk)?;
                self.guard(b, err_blk)?;
                let rhs = self.expr(b, fe, value, epilogue, err_blk)?;
                self.guard(b, err_blk)?;
                let (np, nl) = self.str_ptr(b, name)?;
                let newv = if op == AssignOp::Eq {
                    rhs
                } else {
                    let old = self.rcall(b, "plix_member", 3, &[o, np, nl])?;
                    self.guard(b, err_blk)?;
                    let r = self.rt_binop(b, compound(op), old, rhs)?;
                    self.guard(b, err_blk)?;
                    r
                };
                let r2 = self.rcall(b, "plix_member_set", 4, &[o, np, nl, newv])?;
                self.guard(b, err_blk)?;
                Ok(r2)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// driver methods on Compiler
// ---------------------------------------------------------------------------

/// call a runtime function (without needing a full Emit)
fn rcall_direct(
    shared: &mut Shared,
    fb: &mut FunctionBuilder,
    name: &'static str,
    nparams: usize,
    args: &[CVal],
) -> CResult<CVal> {
    let fid = shared.rt_id(name, nparams)?;
    let fr = shared.module.declare_func_in_func(fid, fb.func);
    let call = fb.ins().call(fr, args);
    Ok(fb.inst_results(call)[0])
}

impl Compiler {
    /// shared tail of every compiled function: fallthrough + error block +
    /// epilogue (release locals, pop frame, return)
    fn finish_body(
        shared: &mut Shared,
        fb: &mut FunctionBuilder,
        locals_all: &[Variable],
        ret_var: Variable,
        epilogue: Block,
        err_blk: Block,
        trace_name: &str,
    ) -> CResult<()> {
        // normal fallthrough
        fb.ins().jump(epilogue, &[]);
        // error block: record this frame on the error trace (zero cost on
        // the success path), then return null with the error flag set
        fb.switch_to_block(err_blk);
        fb.seal_block(err_blk);
        if !trace_name.is_empty() {
            let did = shared.str_data(trace_name)?;
            let gv = shared.module.declare_data_in_func(did, fb.func);
            let p = fb.ins().global_value(I64, gv);
            let l = fb.ins().iconst(I64, trace_name.len() as i64);
            rcall_direct(shared, fb, "plix_trace_push", 2, &[p, l])?;
        }
        let nul = fb.ins().iconst(I64, TNULL);
        fb.def_var(ret_var, nul);
        fb.ins().jump(epilogue, &[]);
        // epilogue
        fb.switch_to_block(epilogue);
        fb.seal_block(epilogue);
        for v in locals_all {
            let cur = fb.use_var(*v);
            rcall_direct(shared, fb, "plix_release", 1, &[cur])?;
        }
        let ret_raw = fb.use_var(ret_var);
        let outv = rcall_direct(shared, fb, "plix_frame_pop", 1, &[ret_raw])?;
        fb.ins().return_(&[outv]);
        Ok(())
    }

    fn compile_function(
        &mut self,
        id: usize,
        def: &Rc<FuncDef>,
        res: &Resolution,
        tinfo: &crate::typecheck::TypeInfo,
    ) -> CResult<()> {
        let fid = self.fns[&id];
        let fnres = res
            .fns
            .get(&id)
            .ok_or_else(|| format!("internal: missing resolution for {}", def.name))?
            .clone();

        self.ctx.clear();
        self.ctx.func.signature = self.sig.clone();
        let mut fb = FunctionBuilder::new(&mut self.ctx.func, &mut self.fn_ctx);
        let entry = fb.create_block();
        fb.append_block_params_for_function_params(entry);
        fb.switch_to_block(entry);
        fb.seal_block(entry);
        let cells_ptr = fb.block_params(entry)[0];
        let args_ptr = fb.block_params(entry)[1];
        let nargs_v = fb.block_params(entry)[2];

        let epilogue = fb.create_block();
        let err_blk = fb.create_block();
        let ret_var = fb.declare_var(I64);
        let cp_var = fb.declare_var(I64);
        let ret_flags = def
            .ret_ty
            .as_ref()
            .map(guard_flags_of_typeexpr)
            .unwrap_or(0);
        let mut fe = FEnv {
            vars: HashMap::new(),
            locals_all: Vec::new(),
            ret_var,
            cp_var,
            cells_ptr,
            loop_stack: Vec::new(),
            ret_flags,
            fn_name: def.name.clone(),
        };
        let nullv = fb.ins().iconst(I64, TNULL);
        fb.def_var(ret_var, nullv);
        fb.def_var(cp_var, nullv);

        let mut em = Emit {
            shared: &mut self.shared,
            fns: &self.fns,
            fn_name_of: &self.fn_name_of,
            assigned: &self.assigned_globals,
            res,
            tinfo,
            mods: &[],
            is_unit: false,
        };
        em.rcall(&mut fb, "plix_frame_push", 0, &[])?;

        // raw-specializable locals: provable scalar type, never captured
        let mut rawmap: HashMap<String, STy> = HashMap::new();
        if let Some(m) = tinfo.local_tys.get(&id) {
            for (n, t) in m {
                if fnres.cell_vars.contains(n) || !fnres.locals.iter().any(|l| l == n) {
                    continue;
                }
                let st = match t {
                    crate::typecheck::Ty::Int => STy::Int,
                    crate::typecheck::Ty::Float => STy::Float,
                    crate::typecheck::Ty::Bool => STy::Bool,
                    _ => continue,
                };
                rawmap.insert(n.clone(), st);
            }
        }

        // captured cells of enclosing functions
        for (i, name) in fnres.captures.iter().enumerate() {
            fe.vars.insert(name.clone(), Loc::Free(i));
        }
        // pre-declare every local as null (locals never shadow captures)
        for name in &fnres.locals {
            if fe.vars.contains_key(name) {
                continue;
            }
            if let Some(st) = rawmap.get(name) {
                // unboxed representation; initialized to a neutral zero
                match st {
                    STy::Int => {
                        let v = fb.declare_var(I64);
                        let z = fb.ins().iconst(I64, 0);
                        fb.def_var(v, z);
                        fe.vars.insert(name.clone(), Loc::RawInt(v));
                    }
                    STy::Float => {
                        let v = fb.declare_var(F64_TY);
                        let z = fb.ins().f64const(0.0);
                        fb.def_var(v, z);
                        fe.vars.insert(name.clone(), Loc::RawFloat(v));
                    }
                    STy::Bool => {
                        let v = fb.declare_var(I64);
                        let z = fb.ins().iconst(I64, 0);
                        fb.def_var(v, z);
                        fe.vars.insert(name.clone(), Loc::RawBool(v));
                    }
                    STy::Box => unreachable!(),
                }
                continue; // raw locals hold no heap reference: no epilogue release
            }
            let v = fb.declare_var(I64);
            let z = fb.ins().iconst(I64, TNULL);
            fb.def_var(v, z);
            fe.locals_all.push(v);
            if fnres.cell_vars.contains(name) {
                fe.vars.insert(name.clone(), Loc::Cell(v));
            } else {
                fe.vars.insert(name.clone(), Loc::Local(v));
            }
        }

        // bind parameters
        let params = def.params.clone();
        let rest_count = params.iter().filter(|p| p.rest).count();
        for (i, p) in params.iter().enumerate() {
            if p.rest {
                let base = fb.ins().iconst(I64, (i * 8) as i64);
                let ptr = fb.ins().iadd(args_ptr, base);
                let ic = fb.ins().iconst(I64, i as i64);
                let cnt0 = fb.ins().isub(nargs_v, ic);
                let zero = fb.ins().iconst(I64, 0);
                let is_neg = fb.ins().icmp(IntCC::SignedLessThan, cnt0, zero);
                let cnt = fb.ins().select(is_neg, zero, cnt0);
                let arr = em.rcall(&mut fb, "plix_array_new", 2, &[ptr, cnt])?;
                em.guard(&mut fb, err_blk)?;
                em.declare_local(&mut fb, &mut fe, &p.name, arr)?;
                continue;
            }
            // value = nargs > i ? args[i] : default / arity error
            let ok_blk = fb.create_block();
            let dft_blk = fb.create_block();
            let merge_blk = fb.create_block();
            let raw_loc = match rawmap.get(&p.name) {
                Some(st) => Some((*st, fe.vars[&p.name])),
                None => None,
            };
            let pvar = if let Some((_, loc)) = raw_loc {
                match loc {
                    Loc::RawInt(v) | Loc::RawFloat(v) | Loc::RawBool(v) => v,
                    _ => unreachable!(),
                }
            } else {
                fb.declare_var(I64)
            };
            let idxc = fb.ins().iconst(I64, i as i64);
            let has = fb.ins().icmp(IntCC::SignedGreaterThan, nargs_v, idxc);
            fb.ins().brif(has, ok_blk, &[], dft_blk, &[]);
            fb.switch_to_block(ok_blk);
            fb.seal_block(ok_blk);
            let argval = fb
                .ins()
                .load(I64, MemFlags::trusted(), args_ptr, (i * 8) as i32);
            let argval = em.rcall(&mut fb, "plix_var_use", 1, &[argval])?;
            match raw_loc {
                Some((st, _)) => {
                    // hard boundary guard: a wrong runtime type raises here,
                    // the typed body never observes a bad bit pattern
                    let ploc = fe.vars[&p.name];
                    let what = format!("argument \"{}\" of {}", p.name, def.name);
                    let raw = match st {
                        STy::Int => em.unbox_int_guard(&mut fb, argval, err_blk, &what)?,
                        STy::Float => em.unbox_float_guard(&mut fb, argval, err_blk, &what)?,
                        STy::Bool => em.rcall(&mut fb, "plix_truthy", 1, &[argval])?,
                        STy::Box => unreachable!(),
                    };
                    let _ = ploc;
                    fb.def_var(pvar, raw);
                }
                None => {
                    fb.def_var(pvar, argval);
                }
            }
            fb.ins().jump(merge_blk, &[]);
            fb.switch_to_block(dft_blk);
            if let Some(d) = &p.default {
                match raw_loc {
                    Some((st, _)) => {
                        let want = match st {
                            STy::Int => Want::Int,
                            STy::Float => Want::Float,
                            STy::Bool => Want::Bool,
                            STy::Box => unreachable!(),
                        };
                        let raw =
                            em.emit_typed(&mut fb, &mut fe, d, want, err_blk, epilogue, &p.name)?;
                        fb.def_var(pvar, raw);
                    }
                    None => {
                        let dv = em.expr(&mut fb, &mut fe, d, epilogue, err_blk)?;
                        em.guard(&mut fb, err_blk)?;
                        fb.def_var(pvar, dv);
                    }
                }
                fb.ins().jump(merge_blk, &[]);
            } else {
                let msg = format!(
                    "{}: missing argument \"{}\" (too few arguments)",
                    def.name, p.name
                );
                em.error_with_str(&mut fb, &msg)?;
                fb.ins().jump(err_blk, &[]);
            }
            fb.seal_block(dft_blk);
            fb.switch_to_block(merge_blk);
            fb.seal_block(merge_blk);
            if raw_loc.is_none() {
                let merged = fb.use_var(pvar);
                em.declare_local(&mut fb, &mut fe, &p.name, merged)?;
            }
        }
        // extra args check (only when no rest param)
        if rest_count == 0 {
            let np = params.len() as i64;
            let npc = fb.ins().iconst(I64, np);
            let too_many = fb.ins().icmp(IntCC::SignedGreaterThan, nargs_v, npc);
            let ok_blk2 = fb.create_block();
            let errb2 = fb.create_block();
            fb.ins().brif(too_many, errb2, &[], ok_blk2, &[]);
            fb.switch_to_block(errb2);
            fb.seal_block(errb2);
            let msg = format!("{}: too many arguments (expected {})", def.name, np);
            em.error_with_str(&mut fb, &msg)?;
            fb.ins().jump(err_blk, &[]);
            fb.switch_to_block(ok_blk2);
            fb.seal_block(ok_blk2);
        }

        let body: Vec<Stmt> = def.body.to_vec();
        for s in &body {
            em.stmt(&mut fb, &mut fe, s, epilogue, err_blk)?;
        }
        let locals = fe.locals_all.clone();
        let rv = fe.ret_var;
        let tname = def.name.clone();
        Self::finish_body(
            &mut em.shared,
            &mut fb,
            &locals,
            rv,
            epilogue,
            err_blk,
            &tname,
        )?;
        drop(em);
        fb.seal_all_blocks();
        fb.finalize();
        self.shared
            .module
            .define_function(fid, &mut self.ctx)
            .map_err(|e| format!("define fn {}: {}", def.name, e))?;
        Ok(())
    }

    fn compile_unit_pseudo_fn(
        &mut self,
        fid: FuncId,
        stmts: &[Stmt],
        res: &Resolution,
        tinfo: &crate::typecheck::TypeInfo,
        flag_idx: usize,
        mods: &[ModMeta],
        trace_name: &str,
    ) -> CResult<()> {
        let fnres = res
            .fns
            .get(&MAIN_RES_ID)
            .ok_or("internal: missing main resolution")?
            .clone();
        let is_main = flag_idx == usize::MAX;

        self.ctx.clear();
        self.ctx.func.signature = self.sig.clone();
        let mut fb = FunctionBuilder::new(&mut self.ctx.func, &mut self.fn_ctx);
        let entry = fb.create_block();
        fb.append_block_params_for_function_params(entry);
        fb.switch_to_block(entry);
        fb.seal_block(entry);
        let cells_ptr = fb.block_params(entry)[0];

        let epilogue = fb.create_block();
        let err_blk = fb.create_block();
        let ret_var = fb.declare_var(I64);
        let cp_var = fb.declare_var(I64);
        let mut fe = FEnv {
            vars: HashMap::new(),
            locals_all: Vec::new(),
            ret_var,
            cp_var,
            cells_ptr,
            loop_stack: Vec::new(),
            ret_flags: 0,
            fn_name: String::new(),
        };
        let nullv = fb.ins().iconst(I64, TNULL);
        fb.def_var(ret_var, nullv);
        fb.def_var(cp_var, nullv);

        let mut em = Emit {
            shared: &mut self.shared,
            fns: &self.fns,
            fn_name_of: &self.fn_name_of,
            assigned: &self.assigned_globals,
            res,
            tinfo,
            mods,
            is_unit: true,
        };
        em.rcall(&mut fb, "plix_frame_push", 0, &[])?;

        // init-once guard for modules
        if !is_main {
            let idx = fb.ins().iconst(I64, flag_idx as i64);
            let fl = em.rcall(&mut fb, "plix_global_get", 1, &[idx])?;
            let t = em.rcall(&mut fb, "plix_truthy", 1, &[fl])?;
            let run_blk = fb.create_block();
            fb.ins().brif(t, epilogue, &[], run_blk, &[]);
            fb.switch_to_block(run_blk);
            fb.seal_block(run_blk);
            let idx2 = fb.ins().iconst(I64, flag_idx as i64);
            let truev = fb.ins().iconst(I64, TTRUE);
            em.rcall(&mut fb, "plix_global_set", 2, &[idx2, truev])?;
        }

        // pre-declare the pseudo-fn locals (depth > 0 names)
        for name in &fnres.locals {
            let v = fb.declare_var(I64);
            let z = fb.ins().iconst(I64, TNULL);
            fb.def_var(v, z);
            fe.locals_all.push(v);
            if fnres.cell_vars.contains(name) {
                fe.vars.insert(name.clone(), Loc::Cell(v));
            } else {
                fe.vars.insert(name.clone(), Loc::Local(v));
            }
        }

        let body: Vec<Stmt> = stmts.to_vec();
        for s in &body {
            em.stmt(&mut fb, &mut fe, s, epilogue, err_blk)?;
        }
        let locals = fe.locals_all.clone();
        let rv = fe.ret_var;
        Self::finish_body(
            &mut em.shared,
            &mut fb,
            &locals,
            rv,
            epilogue,
            err_blk,
            trace_name,
        )?;
        drop(em);
        fb.seal_all_blocks();
        fb.finalize();
        self.shared
            .module
            .define_function(fid, &mut self.ctx)
            .map_err(|e| format!("define unit fn: {}", e))?;
        Ok(())
    }

    fn compile_c_main(&mut self, plix_main: FuncId, total_globals: i64) -> CResult<()> {
        let sig = make_sig(&[], &[types::I32]);
        let fid = self
            .shared
            .module
            .declare_function("main", Linkage::Export, &sig)
            .map_err(|e| e.to_string())?;
        self.ctx.clear();
        self.ctx.func.signature = sig;
        let mut fb = FunctionBuilder::new(&mut self.ctx.func, &mut self.fn_ctx);
        let entry = fb.create_block();
        fb.switch_to_block(entry);
        let n = fb.ins().iconst(I64, total_globals);
        let fid_init = self.shared.rt_id("plix_rt_init", 1)?;
        let fr = self.shared.module.declare_func_in_func(fid_init, fb.func);
        fb.ins().call(fr, &[n]);
        let fid_ib = self.shared.rt_id("plix_install_builtins", 0)?;
        let fr = self.shared.module.declare_func_in_func(fid_ib, fb.func);
        fb.ins().call(fr, &[]);
        let nullv = fb.ins().iconst(I64, TNULL);
        let zero = fb.ins().iconst(I64, 0);
        let fr = self.shared.module.declare_func_in_func(plix_main, fb.func);
        fb.ins().call(fr, &[nullv, nullv, zero]);
        let fid_ef = self.shared.rt_id("plix_err_flag", 0)?;
        let fr = self.shared.module.declare_func_in_func(fid_ef, fb.func);
        let call = fb.ins().call(fr, &[]);
        let flag = fb.inst_results(call)[0];
        let fail_blk = fb.create_block();
        let ok_blk = fb.create_block();
        fb.ins().brif(flag, fail_blk, &[], ok_blk, &[]);
        fb.switch_to_block(fail_blk);
        fb.seal_block(fail_blk);
        let fid_pe = self.shared.rt_id("plix_print_error", 0)?;
        let fr = self.shared.module.declare_func_in_func(fid_pe, fb.func);
        fb.ins().call(fr, &[]);
        let one32 = fb.ins().iconst(types::I32, 1);
        fb.ins().return_(&[one32]);
        fb.switch_to_block(ok_blk);
        fb.seal_block(ok_blk);
        let zero32 = fb.ins().iconst(types::I32, 0);
        fb.ins().return_(&[zero32]);
        fb.seal_block(entry);
        fb.seal_all_blocks();
        fb.finalize();
        self.shared
            .module
            .define_function(fid, &mut self.ctx)
            .map_err(|e| format!("define main: {}", e))?;
        self.ctx.clear();
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// AST walkers
// ---------------------------------------------------------------------------

fn collect_all_fn_defs(stmts: &[Stmt], out: &mut Vec<Rc<FuncDef>>) {
    fn rec_s(s: &Stmt, out: &mut Vec<Rc<FuncDef>>) {
        match &s.node {
            StmtKind::Func(f) => {
                out.push(f.clone());
                rec_stmts(&f.body, out);
            }
            StmtKind::Struct { fields, .. } => {
                for f in fields {
                    if let Some(d) = &f.default {
                        rec_e(d, out);
                    }
                }
            }
            StmtKind::Impl { methods, .. } | StmtKind::Trait { methods, .. } => {
                for m in methods {
                    out.push(m.clone());
                    rec_stmts(&m.body, out);
                }
            }
            StmtKind::Enum { .. } => {}
            StmtKind::Var { value, .. } => rec_e(value, out),
            StmtKind::ExprStmt(e) => rec_e(e, out),
            StmtKind::Block(b) => rec_stmts(b, out),
            StmtKind::If { cond, then, els } => {
                rec_e(cond, out);
                rec_s(then, out);
                if let Some(e) = els {
                    rec_s(e, out);
                }
            }
            StmtKind::While { cond, body } => {
                rec_e(cond, out);
                rec_s(body, out);
            }
            StmtKind::ForC {
                init,
                cond,
                step,
                body,
            } => {
                if let Some(i) = init {
                    rec_s(i, out);
                }
                if let Some(c) = cond {
                    rec_e(c, out);
                }
                if let Some(st) = step {
                    rec_e(st, out);
                }
                rec_s(body, out);
            }
            StmtKind::ForIn { iter, body, .. } => {
                rec_e(iter, out);
                rec_s(body, out);
            }
            StmtKind::MatchStmt { subject, arms } => {
                rec_e(subject, out);
                for a in arms {
                    match &a.body {
                        MatchBody::Expr(e) => rec_e(e, out),
                        MatchBody::Block(b) => rec_stmts(b, out),
                    }
                }
            }
            StmtKind::Return(e) => {
                if let Some(x) = e {
                    rec_e(x, out);
                }
            }
            _ => {}
        }
    }
    fn rec_stmts(stmts: &[Stmt], out: &mut Vec<Rc<FuncDef>>) {
        for s in stmts {
            rec_s(s, out);
        }
    }
    fn rec_e(e: &Expr, out: &mut Vec<Rc<FuncDef>>) {
        match &e.node {
            ExprKind::FuncLit(f) => {
                out.push(f.clone());
                rec_stmts(&f.body, out);
            }
            ExprKind::Array(xs) => {
                for x in xs {
                    rec_e(x, out);
                }
            }
            ExprKind::Object(ps) => {
                for (_, x) in ps {
                    rec_e(x, out);
                }
            }
            ExprKind::StructLit { fields, .. } => {
                for (_, x) in fields {
                    rec_e(x, out);
                }
            }
            ExprKind::Unary(_, x) | ExprKind::Borrow { expr: x, .. } => rec_e(x, out),
            ExprKind::Binary(_, a, b) | ExprKind::Logical(_, a, b) => {
                rec_e(a, out);
                rec_e(b, out);
            }
            ExprKind::Ternary(a, b, c) => {
                rec_e(a, out);
                rec_e(b, out);
                rec_e(c, out);
            }
            ExprKind::Assign { target, value, .. } => {
                rec_e(value, out);
                match target {
                    AssignTarget::Index(o, i) => {
                        rec_e(o, out);
                        rec_e(i, out);
                    }
                    AssignTarget::Member(o, _) => rec_e(o, out),
                    _ => {}
                }
            }
            ExprKind::Call(c, xs) => {
                rec_e(c, out);
                for x in xs {
                    rec_e(x, out);
                }
            }
            ExprKind::Index(a, b) => {
                rec_e(a, out);
                rec_e(b, out);
            }
            ExprKind::Slice { obj, start, end } => {
                rec_e(obj, out);
                if let Some(s) = start {
                    rec_e(s, out);
                }
                if let Some(e2) = end {
                    rec_e(e2, out);
                }
            }
            ExprKind::Member(o, _) => rec_e(o, out),
            ExprKind::Match { subject, arms } => {
                rec_e(subject, out);
                for a in arms {
                    if let MatchBody::Expr(ex) = &a.body {
                        rec_e(ex, out);
                    }
                }
            }
            _ => {}
        }
    }
    rec_stmts(stmts, out);
}

/// names assigned anywhere in main (used to disable the direct-call
/// optimization for bindings that change after declaration)
fn collect_assigned_idents(stmts: &[Stmt], out: &mut HashSet<String>) {
    fn rec_s(s: &Stmt, out: &mut HashSet<String>) {
        match &s.node {
            StmtKind::Func(f) => walk(&f.body, out),
            StmtKind::Impl { methods, .. } | StmtKind::Trait { methods, .. } => {
                for m in methods {
                    walk(&m.body, out);
                }
            }
            StmtKind::Struct { fields, .. } => {
                for f in fields {
                    if let Some(d) = &f.default {
                        rec_e(d, out);
                    }
                }
            }
            StmtKind::Enum { .. } => {}
            StmtKind::Var { value, .. } => rec_e(value, out),
            StmtKind::ExprStmt(e) => rec_e(e, out),
            StmtKind::Block(b) => walk(b, out),
            StmtKind::If { cond, then, els } => {
                rec_e(cond, out);
                rec_s(then, out);
                if let Some(e) = els {
                    rec_s(e, out);
                }
            }
            StmtKind::While { cond, body } => {
                rec_e(cond, out);
                rec_s(body, out);
            }
            StmtKind::ForC {
                init,
                cond,
                step,
                body,
            } => {
                if let Some(i) = init {
                    rec_s(i, out);
                }
                if let Some(c) = cond {
                    rec_e(c, out);
                }
                if let Some(st) = step {
                    rec_e(st, out);
                }
                rec_s(body, out);
            }
            StmtKind::ForIn { iter, body, .. } => {
                rec_e(iter, out);
                rec_s(body, out);
            }
            StmtKind::MatchStmt { subject, arms } => {
                rec_e(subject, out);
                for a in arms {
                    match &a.body {
                        MatchBody::Expr(e) => rec_e(e, out),
                        MatchBody::Block(b) => walk(b, out),
                    }
                }
            }
            StmtKind::Return(e) => {
                if let Some(x) = e {
                    rec_e(x, out);
                }
            }
            _ => {}
        }
    }
    fn walk(stmts: &[Stmt], out: &mut HashSet<String>) {
        for s in stmts {
            rec_s(s, out);
        }
    }
    fn rec_e(e: &Expr, out: &mut HashSet<String>) {
        match &e.node {
            ExprKind::Assign { target, value, .. } => {
                if let AssignTarget::Ident(n) = target {
                    out.insert(n.clone());
                }
                rec_e(value, out);
            }
            ExprKind::FuncLit(f) => walk(&f.body, out),
            ExprKind::Array(xs) => {
                for x in xs {
                    rec_e(x, out);
                }
            }
            ExprKind::Object(ps) => {
                for (_, x) in ps {
                    rec_e(x, out);
                }
            }
            ExprKind::StructLit { fields, .. } => {
                for (_, x) in fields {
                    rec_e(x, out);
                }
            }
            ExprKind::Unary(_, x) | ExprKind::Borrow { expr: x, .. } => rec_e(x, out),
            ExprKind::Binary(_, a, b) | ExprKind::Logical(_, a, b) => {
                rec_e(a, out);
                rec_e(b, out);
            }
            ExprKind::Ternary(a, b, c) => {
                rec_e(a, out);
                rec_e(b, out);
                rec_e(c, out);
            }
            ExprKind::Call(c, xs) => {
                rec_e(c, out);
                for x in xs {
                    rec_e(x, out);
                }
            }
            ExprKind::Index(a, b) => {
                rec_e(a, out);
                rec_e(b, out);
            }
            ExprKind::Slice { obj, start, end } => {
                rec_e(obj, out);
                if let Some(s) = start {
                    rec_e(s, out);
                }
                if let Some(e2) = end {
                    rec_e(e2, out);
                }
            }
            ExprKind::Member(o, _) => rec_e(o, out),
            ExprKind::Match { subject, arms } => {
                rec_e(subject, out);
                for a in arms {
                    if let MatchBody::Expr(ex) = &a.body {
                        rec_e(ex, out);
                    }
                }
            }
            _ => {}
        }
    }
    for s in stmts {
        rec_s(s, out);
    }
}

// ---------------------------------------------------------------------------
// linking
// ---------------------------------------------------------------------------

/// the whole plixrt static archive, embedded by build.rs (the toolchain IS
/// its own distribution: `plix build` needs no Rust toolchain on the
/// target machine, only the platform C linker)
#[cfg(not(target_env = "msvc"))]
static LIBPLIXRT_A: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/libplixrt.a"));
#[cfg(target_env = "msvc")]
static LIBPLIXRT_A: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/plixrt_embed.lib"));

/// archive file name when extracted for linking
fn rt_archive_name() -> &'static str {
    if cfg!(target_env = "msvc") {
        "plixrt_embed.lib"
    } else {
        "libplixrt.a"
    }
}

/// invoke the platform C toolchain linker to produce a standalone executable
#[cfg(not(windows))]
fn link_executable(obj: &[u8], out: &str) -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!("plix_build_{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let opath = dir.join("plix_main.o");
    let apath = dir.join(rt_archive_name());
    std::fs::write(&opath, obj).map_err(|e| e.to_string())?;
    std::fs::write(&apath, LIBPLIXRT_A).map_err(|e| e.to_string())?;
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
    // platform link libraries: dl is glibc-specific (libSystem on macOS
    // already provides dlopen), pthread is implicit in modern libSystem
    let platform_libs: &[&str] = if cfg!(target_os = "macos") {
        &["-lm"]
    } else {
        &["-lm", "-lpthread", "-ldl"]
    };
    let st = std::process::Command::new(&cc)
        .arg("-o")
        .arg(out)
        .arg(&opath)
        .arg(&apath)
        .args(platform_libs)
        .status()
        .map_err(|e| format!("cannot invoke C linker \"{}\": {}", cc, e))?;
    // temp dir holds a ~23MB copy of libplixrt.a; never leave it behind,
    // /tmp is often a small tmpfs and repeated builds would fill it.
    let _ = std::fs::remove_dir_all(&dir);
    if !st.success() {
        return Err(format!("linking failed (cc exited with {:?})", st.code()));
    }
    Ok(())
}

/// Windows: prefer MSVC link.exe; fall back to a MinGW-style cc/gcc on PATH.
#[cfg(windows)]
fn link_executable(obj: &[u8], out: &str) -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!("plix_build_{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let opath = dir.join("plix_main.obj");
    let apath = dir.join(rt_archive_name());
    std::fs::write(&opath, obj).map_err(|e| e.to_string())?;
    std::fs::write(&apath, LIBPLIXRT_A).map_err(|e| e.to_string())?;
    let out_exe = if out.ends_with(".exe") {
        out.to_string()
    } else {
        format!("{}.exe", out)
    };

    // MSVC developer environment? link.exe on PATH.
    if let Ok(st) = std::process::Command::new("link.exe")
        .arg("/NOLOGO")
        .arg(&opath)
        .arg(&apath)
        .arg(format!("/OUT:{}", out_exe))
        .status()
    {
        let _ = std::fs::remove_dir_all(&dir);
        if st.success() {
            return Ok(());
        }
        return Err(format!(
            "linking failed (link.exe exited with {:?})",
            st.code()
        ));
    }

    // MinGW fallback: cc/gcc with GNU-style archive and standard libs.
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
    let st = std::process::Command::new(&cc)
        .arg("-o")
        .arg(&out_exe)
        .arg(&opath)
        .arg(&apath)
        .args([
            "-lws2_32",
            "-ladvapi32",
            "-lbcrypt",
            "-luserenv",
            "-lkernel32",
        ])
        .status()
        .map_err(|e| {
            format!(
                "no C linker found: install Visual Studio Build Tools (MSVC) or MinGW-w64, \
                 or set CC to a MinGW gcc ({})",
                e
            )
        })?;
    let _ = std::fs::remove_dir_all(&dir);
    if !st.success() {
        return Err(format!(
            "linking failed ({} exited with {:?})",
            cc,
            st.code()
        ));
    }
    Ok(())
}
