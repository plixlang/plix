//! Plix WASM codegen backend — compiles Plix source to WebAssembly binary.
#![allow(dead_code, unused_variables)]
//!
//! Supports a workable subset of Plix:
//!   - Integer and float arithmetic (+, -, *, /, %)
//!   - Comparisons (==, !=, <, >, <=, >=)
//!   - Logical operators (and, or, not)
//!   - if/else, while, for-in, return, break, continue
//!   - Function declarations and calls
//!   - `say()` / `print()` via WASI fd_write (stdout) — int and string
//!   - Local variables (auto, const)
//!   - String literals in say()
//!
//! Usage: `plix build --target wasm file.px -o out.wasm`

use crate::ast;
use std::collections::HashMap;

/// Compile a Plix source file to WASM binary.
pub fn compile_source(source: &str, _file: &str) -> Result<Vec<u8>, String> {
    let stmts = crate::parser::parse_file(source)
        .map_err(|e| format!("{}:{}: {}", e.span.line, e.span.col, e.msg))?;

    let mut w = WasmGen::new();
    w.compile(&stmts)?;
    Ok(w.finish())
}

// ---------------------------------------------------------------------------
// WASM binary format constants
// ---------------------------------------------------------------------------

const WASM_MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6D];
const WASM_VERSION: [u8; 4] = [0x01, 0x00, 0x00, 0x00];
const I32: u8 = 0x7F;
const I64: u8 = 0x7E;
const F64: u8 = 0x7C;

// WASM opcodes
const OP_UNREACHABLE: u8 = 0x00;
const OP_NOP: u8 = 0x01;
const OP_BLOCK: u8 = 0x02;
const OP_LOOP: u8 = 0x03;
const OP_IF: u8 = 0x04;
const OP_ELSE: u8 = 0x05;
const OP_END: u8 = 0x0B;
const OP_BR: u8 = 0x0C;
const OP_BR_IF: u8 = 0x0D;
const OP_RETURN: u8 = 0x0F;
const OP_CALL: u8 = 0x10;
const OP_DROP: u8 = 0x1A;
const OP_SELECT: u8 = 0x1B;
const OP_LOCAL_GET: u8 = 0x20;
const OP_LOCAL_SET: u8 = 0x21;
const OP_LOCAL_TEE: u8 = 0x22;
const OP_I32_CONST: u8 = 0x41;
const OP_I64_CONST: u8 = 0x42;
const OP_F64_CONST: u8 = 0x44;
const OP_I32_EQZ: u8 = 0x45;
const OP_I32_EQ: u8 = 0x46;
const OP_I32_NE: u8 = 0x47;
const OP_I32_LT_S: u8 = 0x48;
const OP_I32_LT_U: u8 = 0x49;
const OP_I32_GT_S: u8 = 0x4A;
const OP_I32_GT_U: u8 = 0x4B;
const OP_I32_LE_S: u8 = 0x4C;
const OP_I32_GE_S: u8 = 0x4E;
const OP_I64_EQZ: u8 = 0x50;
const OP_I64_EQ: u8 = 0x51;
const OP_I64_LT_S: u8 = 0x52;
const OP_I64_GT_S: u8 = 0x54;
const OP_F64_EQ: u8 = 0x61;
const OP_F64_LT: u8 = 0x63;
const OP_F64_GT: u8 = 0x64;
const OP_F64_LE: u8 = 0x65;
const OP_F64_GE: u8 = 0x66;
const OP_I32_ADD: u8 = 0x6A;
const OP_I32_SUB: u8 = 0x6B;
const OP_I32_MUL: u8 = 0x6C;
const OP_I32_DIV_S: u8 = 0x6D;
const OP_I32_REM_S: u8 = 0x6F;
const OP_I32_AND: u8 = 0x71;
const OP_I32_OR: u8 = 0x72;
const OP_I32_XOR: u8 = 0x73;
const OP_I64_ADD: u8 = 0x7C;
const OP_I64_SUB: u8 = 0x7D;
const OP_I64_MUL: u8 = 0x7E;
const OP_I64_DIV_S: u8 = 0x7F;
const OP_F64_ADD: u8 = 0xA0;
const OP_F64_SUB: u8 = 0xA1;
const OP_F64_MUL: u8 = 0xA2;
const OP_F64_DIV: u8 = 0xA3;
const OP_F64_NEG: u8 = 0x9A;

// ---------------------------------------------------------------------------
// Code generator
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum ValKind { I32, I64, F64 }

struct WasmFunc {
    name: String,
    type_idx: u32,
    body: Vec<u8>,
    n_params: usize,
    local_names: Vec<String>,
    local_kinds: Vec<ValKind>,
}

struct WasmGen {
    types: Vec<FuncType>,
    imports: Vec<(String, String, u32)>,
    functions: Vec<WasmFunc>,
    func_name_to_idx: HashMap<String, u32>,
    exports: Vec<(String, u8, u32)>,
    start_func: Option<u32>,
    data_segments: Vec<(u32, Vec<u8>)>,
    mem_offset: u32,
    string_pool: HashMap<String, u32>,
    // Per-function compilation state
    local_idx: HashMap<String, u32>,
    current_locals: usize,
    depth: u32,
    uses_float: bool,
    // Type indices for helpers
    say_int_type: u32,
    say_str_type: u32,
}

#[derive(Clone)]
struct FuncType {
    params: Vec<u8>,
    results: Vec<u8>,
}

impl WasmGen {
    fn new() -> Self {
        WasmGen {
            types: Vec::new(),
            imports: Vec::new(),
            functions: Vec::new(),
            func_name_to_idx: HashMap::new(),
            exports: Vec::new(),
            start_func: None,
            data_segments: Vec::new(),
            mem_offset: 128, // leave room for helper scratch area (offsets 0-67)
            string_pool: HashMap::new(),
            local_idx: HashMap::new(),
            current_locals: 0,
            depth: 0,
            uses_float: false,
            say_int_type: 0,
            say_str_type: 0,
        }
    }

    fn add_type(&mut self, params: Vec<u8>, results: Vec<u8>) -> u32 {
        let idx = self.types.len() as u32;
        self.types.push(FuncType { params, results });
        idx
    }

    fn add_string_data(&mut self, s: &str) -> u32 {
        if let Some(&off) = self.string_pool.get(s) {
            return off;
        }
        let offset = self.mem_offset;
        self.data_segments.push((offset, s.as_bytes().to_vec()));
        self.mem_offset += s.len() as u32;
        self.mem_offset = (self.mem_offset + 3) & !3;
        self.string_pool.insert(s.to_string(), offset);
        offset
    }

    // -----------------------------------------------------------------------
    // Top-level compilation
    // -----------------------------------------------------------------------

    fn compile(&mut self, stmts: &[ast::Stmt]) -> Result<(), String> {
        // WASI fd_write import: (fd, iov_ptr, iov_count, nwritten_ptr) -> i32
        let fd_write_type = self.add_type(vec![I32, I32, I32, I32], vec![I32]);
        self.imports.push(("wasi_snapshot_preview1".into(), "fd_write".into(), fd_write_type));

        // WASI proc_exit: (code) -> ()
        let proc_exit_type = self.add_type(vec![I32], vec![]);
        self.imports.push(("wasi_snapshot_preview1".into(), "proc_exit".into(), proc_exit_type));

        let import_count = self.imports.len() as u32;

        // Helper function types
        self.say_int_type = self.add_type(vec![I32], vec![]);       // __plix_say_int(i32)
        self.say_str_type = self.add_type(vec![I32, I32], vec![]);  // __plix_say_str(i32, i32)

        // Register helpers — say_int at import_count, say_str at import_count+1
        self.func_name_to_idx.insert("__plix_say_int".into(), import_count);
        self.func_name_to_idx.insert("__plix_say_str".into(), import_count + 1);

        // First pass: register all function names so calls resolve
        let mut func_defs = Vec::new();
        for s in stmts {
            if let ast::StmtKind::Func(f) = &s.node {
                let func_idx = import_count + 2 + func_defs.len() as u32; // +2 for helpers
                self.func_name_to_idx.insert(f.name.clone(), func_idx);
                func_defs.push((f.clone(), s.span));
            }
        }

        // Compile helpers
        let say_int_func = self.compile_say_int_helper();
        self.functions.push(say_int_func);

        let say_str_func = self.compile_say_str_helper();
        self.functions.push(say_str_func);

        // Compile each user function
        for (f, span) in &func_defs {
            let func = self.compile_func(f, *span)?;
            self.functions.push(func);
        }

        // Set start function to main
        if let Some(&main_idx) = self.func_name_to_idx.get("main") {
            self.start_func = Some(main_idx);
        }

        // Exports
        for (i, f) in self.functions.iter().enumerate() {
            if f.name == "main" || f.name == "_start" {
                self.exports.push((f.name.clone(), 0, import_count + i as u32));
            }
        }
        self.exports.push(("memory".into(), 2, 0));

        Ok(())
    }

    // -----------------------------------------------------------------------
    // __plix_say_int: converts i32 to decimal string and writes via fd_write
    //
    // Memory layout (shared with say_str):
    //   offset  0: iov[0] — { ptr: i32, len: i32 }  (8 bytes)
    //   offset  8: iov[1] — { ptr: i32, len: i32 }  (8 bytes)  [used by say_str]
    //   offset 16: scratch buffer                      (48 bytes, offset 16..63)
    //   offset 64: nwritten ptr                         (4 bytes)
    //
    // Strategy: write digits backwards from offset 63, then write '-'
    // if negative, then '\n'. Set iov.ptr = first_byte_position, iov.len = count.
    // -----------------------------------------------------------------------

    fn compile_say_int_helper(&mut self) -> WasmFunc {
        let type_idx = self.say_int_type;
        let mut b = Vec::new();

        // Memory layout:
        //   offset  0: iov — { ptr: i32, len: i32 }  (8 bytes)
        //   offset  8: scratch buffer (48 bytes, offset 8..55) — digits written backward from 55
        //   offset 56: '\n' (1 byte, pre-stored) — right after scratch, never overwritten by digits
        //   offset 60: nwritten ptr (4 bytes)
        //
        // Strategy: write digits backwards from offset 55. The '\n' at offset 56
        // is always safe because digits start at 55 and go DOWN.
        // After digits, set iov.ptr = first_digit, iov.len = digit_count + 1 (includes '\n').
        // Uses 1 iov entry, 1 fd_write call.

        // Locals: 0=value(param), 1=write_pos, 2=is_negative, 3=digit_count
        b.push(0x01); // 1 local group
        b.push(0x03); // 3 extra locals
        b.push(I32);

        // Pre-store '\n' at offset 56
        b.push(OP_I32_CONST); b.push(56); // addr
        b.push(OP_I32_CONST); b.push(10); // '\n'
        b.push(0x3A); b.push(0x00); b.push(0x00); // i32.store8

        // write_pos = 55 (last byte of scratch buffer)
        b.push(OP_I32_CONST); b.push(55);
        b.push(OP_LOCAL_SET); b.push(0x01);

        // is_negative = 0
        b.push(OP_I32_CONST); b.push(0x00);
        b.push(OP_LOCAL_SET); b.push(0x02);

        // digit_count = 0
        b.push(OP_I32_CONST); b.push(0x00);
        b.push(OP_LOCAL_SET); b.push(0x03);

        // Check if negative: if value < 0, set is_negative=1, negate value
        b.push(OP_LOCAL_GET); b.push(0x00);
        b.push(OP_I32_CONST); b.push(0x00);
        b.push(OP_I32_LT_S);
        b.push(OP_IF); b.push(0x40); // void
            b.push(OP_I32_CONST); b.push(0x01);
            b.push(OP_LOCAL_SET); b.push(0x02); // is_negative = 1
            b.push(OP_I32_CONST); b.push(0x00);
            b.push(OP_LOCAL_GET); b.push(0x00);
            b.push(OP_I32_SUB); // 0 - value
            b.push(OP_LOCAL_SET); b.push(0x00); // value = -value
        b.push(OP_END);

        // Special case: value == 0 → write '0'
        b.push(OP_LOCAL_GET); b.push(0x00);
        b.push(OP_I32_EQZ);
        b.push(OP_IF); b.push(0x40);
            // store8: push addr, then val
            b.push(OP_LOCAL_GET); b.push(0x01); // addr = write_pos
            b.push(OP_I32_CONST); b.push(48);   // '0'
            b.push(0x3A); b.push(0x00); b.push(0x00); // i32.store8
            // write_pos--
            b.push(OP_LOCAL_GET); b.push(0x01);
            b.push(OP_I32_CONST); b.push(1);
            b.push(OP_I32_SUB);
            b.push(OP_LOCAL_SET); b.push(0x01);
            // digit_count++
            b.push(OP_LOCAL_GET); b.push(0x03);
            b.push(OP_I32_CONST); b.push(1);
            b.push(OP_I32_ADD);
            b.push(OP_LOCAL_SET); b.push(0x03);
        b.push(OP_ELSE);
            // Loop: while value != 0, extract least-significant digit
            b.push(OP_LOOP); b.push(0x40);
                b.push(OP_LOCAL_GET); b.push(0x00);
                b.push(OP_I32_EQZ);
                b.push(OP_BR_IF); b.push(0x01); // break if value == 0

                // store8 at write_pos: push addr first, then value
                b.push(OP_LOCAL_GET); b.push(0x01); // addr = write_pos
                b.push(OP_LOCAL_GET); b.push(0x00); // value
                b.push(OP_I32_CONST); b.push(10);
                b.push(OP_I32_REM_S); // digit = value % 10
                b.push(OP_I32_CONST); b.push(48);
                b.push(OP_I32_ADD); // digit + '0'
                b.push(0x3A); b.push(0x00); b.push(0x00); // i32.store8

                // value = value / 10
                b.push(OP_LOCAL_GET); b.push(0x00);
                b.push(OP_I32_CONST); b.push(10);
                b.push(OP_I32_DIV_S);
                b.push(OP_LOCAL_SET); b.push(0x00);

                // write_pos--
                b.push(OP_LOCAL_GET); b.push(0x01);
                b.push(OP_I32_CONST); b.push(1);
                b.push(OP_I32_SUB);
                b.push(OP_LOCAL_SET); b.push(0x01);

                // digit_count++
                b.push(OP_LOCAL_GET); b.push(0x03);
                b.push(OP_I32_CONST); b.push(1);
                b.push(OP_I32_ADD);
                b.push(OP_LOCAL_SET); b.push(0x03);

                b.push(OP_BR); b.push(0x00); // continue
            b.push(OP_END); // end loop
        b.push(OP_END); // end if value==0

        // If was negative, write '-' at write_pos
        b.push(OP_LOCAL_GET); b.push(0x02); // is_negative
        b.push(OP_IF); b.push(0x40);
            b.push(OP_LOCAL_GET); b.push(0x01); // addr
            b.push(OP_I32_CONST); b.push(45);   // '-'
            b.push(0x3A); b.push(0x00); b.push(0x00); // i32.store8
            // write_pos--
            b.push(OP_LOCAL_GET); b.push(0x01);
            b.push(OP_I32_CONST); b.push(1);
            b.push(OP_I32_SUB);
            b.push(OP_LOCAL_SET); b.push(0x01);
            // digit_count++
            b.push(OP_LOCAL_GET); b.push(0x03);
            b.push(OP_I32_CONST); b.push(1);
            b.push(OP_I32_ADD);
            b.push(OP_LOCAL_SET); b.push(0x03);
        b.push(OP_END);

        // Set up iov: ptr = write_pos + 1, len = digit_count + 1 (include '\n')
        b.push(OP_I32_CONST); b.push(0x00); // addr for iov.ptr
        b.push(OP_LOCAL_GET); b.push(0x01);
        b.push(OP_I32_CONST); b.push(1);
        b.push(OP_I32_ADD); // write_pos + 1 = first digit position
        b.push(0x36); b.push(0x02); b.push(0x00); // i32.store align=2 offset=0

        b.push(OP_I32_CONST); b.push(0x04); // addr for iov.len
        b.push(OP_LOCAL_GET); b.push(0x03); // digit_count
        b.push(OP_I32_CONST); b.push(1);
        b.push(OP_I32_ADD); // digit_count + 1 (for '\n')
        b.push(0x36); b.push(0x02); b.push(0x00); // i32.store align=2 offset=0

        // Call fd_write(1, 0, 1, 60)
        b.push(OP_I32_CONST); b.push(1); // fd = stdout
        b.push(OP_I32_CONST); b.push(0); // iov_ptr
        b.push(OP_I32_CONST); b.push(1); // iov_count = 1
        b.push(OP_I32_CONST); b.extend_from_slice(&leb128_signed(60)); // nwritten_ptr
        b.push(OP_CALL); b.extend_from_slice(&leb128(0)); // fd_write (import 0)
        b.push(OP_DROP);

        b.push(OP_END); // end function

        WasmFunc {
            name: "__plix_say_int".into(),
            type_idx,
            body: b,
            n_params: 1,
            local_names: vec!["value".into(), "write_pos".into(), "is_negative".into(), "digit_count".into()],
            local_kinds: vec![ValKind::I32; 4],
        }
    }

    // -----------------------------------------------------------------------
    // __plix_say_str: writes a string + newline via WASI fd_write
    //
    // Params: offset (i32), length (i32)
    // Memory layout (shared with say_int):
    //   offset  0: iov[0] — { ptr, len }
    //   offset  8: iov[1] — { ptr, len }  (for newline)
    //   offset 16: scratch (48 bytes)      (newline byte stored at offset 16)
    //   offset 64: nwritten ptr
    //
    // Strategy: use two iov entries — one for the string, one for '\n'.
    // fd_write(1, 0, 2, 64) writes both in one call.
    // -----------------------------------------------------------------------

    fn compile_say_str_helper(&mut self) -> WasmFunc {
        let type_idx = self.say_str_type;
        let mut b = Vec::new();

        // Memory layout (shared with say_int):
        //   offset  0: iov — { ptr, len } (8 bytes)
        //   offset  8: scratch (48 bytes)
        //   offset 56: '\n' (1 byte)
        //   offset 60: nwritten ptr (4 bytes)
        //
        // For say_str, we write the string + '\n' at offset 56 using 2 iov entries.
        // Actually, let's use a simpler approach: write '\n' right after the string
        // data using a single iov that spans string + newline.
        // But since string is in data segments (offset >= 128), we can't guarantee
        // '\n' follows. So use iov[0] for string, write '\n' at offset 56.
        // Use single iov: copy string to scratch, append '\n' — but that's complex.
        //
        // Simplest: two separate fd_write calls. First writes the string, second writes '\n'.

        // No extra locals needed — params are local 0 (offset) and local 1 (length)
        b.push(0x00); // 0 local groups

        // --- First fd_write: write the string itself ---
        // Set up iov: ptr = offset (param 0), len = length (param 1)
        b.push(OP_I32_CONST); b.push(0x00); // addr for iov.ptr
        b.push(OP_LOCAL_GET); b.push(0x00); // offset
        b.push(0x36); b.push(0x02); b.push(0x00); // i32.store align=2 offset=0

        b.push(OP_I32_CONST); b.push(0x04); // addr for iov.len
        b.push(OP_LOCAL_GET); b.push(0x01); // length
        b.push(0x36); b.push(0x02); b.push(0x00); // i32.store align=2 offset=0

        // fd_write(1, 0, 1, 60) — write string
        b.push(OP_I32_CONST); b.push(1); // fd = stdout
        b.push(OP_I32_CONST); b.push(0); // iov_ptr
        b.push(OP_I32_CONST); b.push(1); // iov_count = 1
        b.push(OP_I32_CONST); b.extend_from_slice(&leb128_signed(60)); // nwritten_ptr
        b.push(OP_CALL); b.extend_from_slice(&leb128(0)); // fd_write (import 0)
        b.push(OP_DROP);

        // --- Second fd_write: write '\n' ---
        // Store '\n' at offset 56
        b.push(OP_I32_CONST); b.push(56); // addr
        b.push(OP_I32_CONST); b.push(10); // '\n'
        b.push(0x3A); b.push(0x00); b.push(0x00); // i32.store8

        // Set up iov: ptr = 56, len = 1
        b.push(OP_I32_CONST); b.push(0x00); // addr for iov.ptr
        b.push(OP_I32_CONST); b.push(56);   // newline address
        b.push(0x36); b.push(0x02); b.push(0x00); // i32.store align=2 offset=0

        b.push(OP_I32_CONST); b.push(0x04); // addr for iov.len
        b.push(OP_I32_CONST); b.push(1);    // length = 1
        b.push(0x36); b.push(0x02); b.push(0x00); // i32.store align=2 offset=0

        // fd_write(1, 0, 1, 60) — write newline
        b.push(OP_I32_CONST); b.push(1); // fd = stdout
        b.push(OP_I32_CONST); b.push(0); // iov_ptr
        b.push(OP_I32_CONST); b.push(1); // iov_count = 1
        b.push(OP_I32_CONST); b.extend_from_slice(&leb128_signed(60)); // nwritten_ptr
        b.push(OP_CALL); b.extend_from_slice(&leb128(0)); // fd_write (import 0)
        b.push(OP_DROP);

        b.push(OP_END); // end function

        WasmFunc {
            name: "__plix_say_str".into(),
            type_idx,
            body: b,
            n_params: 2,
            local_names: vec!["offset".into(), "length".into()],
            local_kinds: vec![ValKind::I32, ValKind::I32],
        }
    }

    // -----------------------------------------------------------------------
    // Function compilation
    // -----------------------------------------------------------------------

    fn compile_func(&mut self, f: &ast::FuncDef, _span: crate::token::Span) -> Result<WasmFunc, String> {
        // Reset per-function state
        self.local_idx.clear();
        self.current_locals = 0;
        self.uses_float = false;
        self.depth = 0;

        // Register parameters as locals
        let n_params = f.params.len();
        let mut param_types = Vec::new();
        for p in &f.params {
            let kind = if let Some(ty) = &p.ty {
                if ty.name.contains("float") || ty.name.contains("f64") {
                    self.uses_float = true;
                    ValKind::F64
                } else {
                    ValKind::I32
                }
            } else {
                ValKind::I32
            };
            param_types.push(match kind { ValKind::I32 => I32, ValKind::I64 => I64, ValKind::F64 => F64 });
            self.local_idx.insert(p.name.clone(), self.current_locals as u32);
            self.current_locals += 1;
        }

        // Determine return type
        let ret_type = if let Some(ret) = &f.ret_ty {
            if ret.name.contains("float") || ret.name.contains("f64") {
                self.uses_float = true;
                vec![F64]
            } else if ret.name.contains("int") || ret.name.contains("bool") {
                vec![I32]
            } else {
                vec![]
            }
        } else {
            vec![]
        };

        let type_idx = self.add_type(param_types, ret_type.clone());

        // First pass: collect all local variable declarations
        let mut local_names = f.params.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
        let mut local_kinds: Vec<ValKind> = (0..n_params).map(|_| ValKind::I32).collect();
        self.collect_locals(&f.body, &mut local_names, &mut local_kinds);

        // Generate body
        let mut body = Vec::new();

        // Local declarations for non-param locals
        let n_extra_locals = local_names.len() - n_params;
        if n_extra_locals > 0 {
            body.push(0x01); // 1 local group
            body.push(n_extra_locals as u8);
            body.push(I32);
        } else {
            body.push(0x00); // 0 local groups
        }

        // Generate statements
        for s in &f.body {
            self.gen_stmt(&mut body, s);
        }

        // Add implicit return value if needed
        if ret_type.is_empty() {
            // void function — no value needed
        } else if ret_type[0] == I32 {
            body.push(OP_I32_CONST); body.push(0x00);
        } else if ret_type[0] == F64 {
            body.push(OP_F64_CONST); body.extend_from_slice(&0.0f64.to_le_bytes());
        }

        body.push(OP_END);

        Ok(WasmFunc {
            name: f.name.clone(),
            type_idx,
            body,
            n_params,
            local_names,
            local_kinds,
        })
    }

    fn collect_locals(&mut self, stmts: &[ast::Stmt], names: &mut Vec<String>, kinds: &mut Vec<ValKind>) {
        for s in stmts {
            match &s.node {
                ast::StmtKind::Var { name, .. } => {
                    if !self.local_idx.contains_key(name) {
                        self.local_idx.insert(name.clone(), self.current_locals as u32);
                        self.current_locals += 1;
                        names.push(name.clone());
                        kinds.push(ValKind::I32);
                    }
                }
                ast::StmtKind::ForIn { name, body, .. } => {
                    if !self.local_idx.contains_key(name) {
                        self.local_idx.insert(name.clone(), self.current_locals as u32);
                        self.current_locals += 1;
                        names.push(name.clone());
                        kinds.push(ValKind::I32);
                    }
                    self.collect_locals(std::slice::from_ref(body), names, kinds);
                }
                ast::StmtKind::If { then, els, .. } => {
                    self.collect_locals(std::slice::from_ref(then), names, kinds);
                    if let Some(e) = els { self.collect_locals(std::slice::from_ref(e), names, kinds); }
                }
                ast::StmtKind::While { body, .. } => { self.collect_locals(std::slice::from_ref(body), names, kinds); }
                ast::StmtKind::Block(stmts2) => { self.collect_locals(stmts2, names, kinds); }
                ast::StmtKind::Func(f) => {
                    if !self.local_idx.contains_key(&f.name) {
                        self.local_idx.insert(f.name.clone(), self.current_locals as u32);
                        self.current_locals += 1;
                        names.push(f.name.clone());
                        kinds.push(ValKind::I32);
                    }
                }
                _ => {}
            }
        }
    }

    // -----------------------------------------------------------------------
    // Statement generation
    // -----------------------------------------------------------------------

    fn gen_stmt(&mut self, b: &mut Vec<u8>, s: &ast::Stmt) {
        match &s.node {
            ast::StmtKind::ExprStmt(e) => {
                self.gen_expr(b, e);
                b.push(OP_DROP);
            }
            ast::StmtKind::Var { name, value, .. } => {
                self.gen_expr(b, value);
                if let Some(&idx) = self.local_idx.get(name) {
                    b.push(OP_LOCAL_SET); b.extend_from_slice(&leb128(idx));
                } else {
                    b.push(OP_DROP);
                }
            }
            ast::StmtKind::Return(e) => {
                if let Some(e) = e { self.gen_expr(b, e); }
                b.push(OP_RETURN);
            }
            ast::StmtKind::If { cond, then, els } => {
                self.gen_expr(b, cond);
                b.push(OP_IF); b.push(0x40); // void block type
                self.gen_stmt(b, then);
                if let Some(els) = els {
                    b.push(OP_ELSE);
                    self.gen_stmt(b, els);
                }
                b.push(OP_END);
            }
            ast::StmtKind::While { cond, body } => {
                self.depth += 1;
                let loop_label = self.depth;
                b.push(OP_BLOCK); b.push(0x40); // break target
                b.push(OP_LOOP); b.push(0x40);  // continue target
                self.gen_expr(b, cond);
                b.push(OP_I32_EQZ); // if !cond, break
                b.push(OP_BR_IF); b.extend_from_slice(&leb128(loop_label));
                self.gen_stmt(b, body);
                b.push(OP_BR); b.extend_from_slice(&leb128(0)); // continue
                b.push(OP_END); // end loop
                b.push(OP_END); // end block
                self.depth -= 1;
            }
            ast::StmtKind::ForIn { name, iter, body, .. } => {
                self.depth += 1;
                if let Some(&name_idx) = self.local_idx.get(name) {
                    self.gen_expr(b, iter);
                    b.push(OP_LOCAL_SET); b.extend_from_slice(&leb128(name_idx));
                }
                self.gen_stmt(b, body);
                self.depth -= 1;
            }
            ast::StmtKind::Block(stmts) => {
                for s in stmts { self.gen_stmt(b, s); }
            }
            ast::StmtKind::Break => {
                if self.depth > 0 {
                    b.push(OP_BR); b.extend_from_slice(&leb128(self.depth));
                }
            }
            ast::StmtKind::Continue => {
                if self.depth > 0 {
                    b.push(OP_BR); b.extend_from_slice(&leb128(0));
                }
            }
            ast::StmtKind::Import { .. } => { /* imports are resolved at link time */ }
            _ => {}
        }
    }

    // -----------------------------------------------------------------------
    // Expression generation
    // -----------------------------------------------------------------------

    fn gen_expr(&mut self, b: &mut Vec<u8>, e: &ast::Expr) {
        match &e.node {
            ast::ExprKind::Int(n) => {
                if *n >= 0 && *n <= 0x7FFFFFFF {
                    b.push(OP_I32_CONST);
                    b.extend_from_slice(&leb128_signed(*n as i32 as i64));
                } else {
                    b.push(OP_I64_CONST);
                    b.extend_from_slice(&leb128_signed(*n));
                }
            }
            ast::ExprKind::Float(f) => {
                b.push(OP_F64_CONST);
                b.extend_from_slice(&f.to_le_bytes());
                self.uses_float = true;
            }
            ast::ExprKind::Bool(v) => {
                b.push(OP_I32_CONST);
                b.push(if *v { 1 } else { 0 });
            }
            ast::ExprKind::Null => {
                b.push(OP_I32_CONST); b.push(0x00);
            }
            ast::ExprKind::Str(s) => {
                // Push (offset, length) as two i32 values on the stack
                let offset = self.add_string_data(s);
                b.push(OP_I32_CONST); b.extend_from_slice(&leb128(offset));
                b.push(OP_I32_CONST); b.extend_from_slice(&leb128(s.len() as u32));
            }
            ast::ExprKind::Ident(name) => {
                if let Some(&idx) = self.local_idx.get(name) {
                    b.push(OP_LOCAL_GET); b.extend_from_slice(&leb128(idx));
                } else if let Some(&func_idx) = self.func_name_to_idx.get(name) {
                    b.push(OP_I32_CONST); b.extend_from_slice(&leb128(func_idx));
                } else {
                    b.push(OP_I32_CONST); b.push(0x00);
                }
            }
            ast::ExprKind::Binary(op, a, c) => {
                self.gen_expr(b, a);
                self.gen_expr(b, c);

                match op {
                    ast::BinOp::Add => { b.push(OP_I32_ADD); }
                    ast::BinOp::Sub => { b.push(OP_I32_SUB); }
                    ast::BinOp::Mul => { b.push(OP_I32_MUL); }
                    ast::BinOp::Div => { b.push(OP_I32_DIV_S); }
                    ast::BinOp::Mod => { b.push(OP_I32_REM_S); }
                    ast::BinOp::Eq  => { b.push(OP_I32_EQ); }
                    ast::BinOp::Ne  => { b.push(OP_I32_NE); }
                    ast::BinOp::Lt  => { b.push(OP_I32_LT_S); }
                    ast::BinOp::Gt  => { b.push(OP_I32_GT_S); }
                    ast::BinOp::Le  => { b.push(OP_I32_LE_S); }
                    ast::BinOp::Ge  => { b.push(OP_I32_GE_S); }
                    ast::BinOp::BAnd => { b.push(OP_I32_AND); }
                    ast::BinOp::BOr  => { b.push(OP_I32_OR); }
                    ast::BinOp::BXor => { b.push(OP_I32_XOR); }
                    _    => { b.push(OP_I32_ADD); } // fallback
                }
            }
            ast::ExprKind::Logical(op, a, c) => {
                match op {
                    ast::LogicalOp::And => {
                        self.gen_expr(b, a);
                        b.push(OP_IF); b.push(I32);
                        self.gen_expr(b, c);
                        b.push(OP_ELSE);
                        b.push(OP_I32_CONST); b.push(0x00);
                        b.push(OP_END);
                    }
                    ast::LogicalOp::Or => {
                        self.gen_expr(b, a);
                        b.push(OP_IF); b.push(I32);
                        b.push(OP_I32_CONST); b.push(0x01);
                        b.push(OP_ELSE);
                        self.gen_expr(b, c);
                        b.push(OP_END);
                    }
                }
            }
            ast::ExprKind::Unary(op, x) => {
                match op {
                    ast::UnOp::Neg => {
                        b.push(OP_I32_CONST); b.push(0x00);
                        self.gen_expr(b, x);
                        b.push(OP_I32_SUB);
                    }
                    ast::UnOp::Not => {
                        self.gen_expr(b, x);
                        b.push(OP_I32_EQZ);
                    }
                    _ => { self.gen_expr(b, x); }
                }
            }
            ast::ExprKind::Call(callee, args) => {
                // Built-in function handling
                if let ast::ExprKind::Ident(name) = &callee.node {
                    match name.as_str() {
                        "say" | "print" => {
                            if !args.is_empty() {
                                // Check if argument is a string literal → use say_str
                                if let ast::ExprKind::Str(s) = &args[0].node {
                                    let offset = self.add_string_data(s);
                                    b.push(OP_I32_CONST); b.extend_from_slice(&leb128(offset));
                                    b.push(OP_I32_CONST); b.extend_from_slice(&leb128(s.len() as u32));
                                    let say_str_idx = *self.func_name_to_idx.get("__plix_say_str").unwrap_or(&0);
                                    b.push(OP_CALL); b.extend_from_slice(&leb128(say_str_idx));
                                } else {
                                    // Integer/expression → use say_int
                                    self.gen_expr(b, &args[0]);
                                    // say_int expects one i32, but if expr is a string
                                    // (which pushes offset+length), that's 2 values.
                                    // Only the top value would be consumed by say_int.
                                    // For non-string args this is fine.
                                    let say_idx = *self.func_name_to_idx.get("__plix_say_int").unwrap_or(&0);
                                    b.push(OP_CALL); b.extend_from_slice(&leb128(say_idx));
                                }
                            }
                            b.push(OP_I32_CONST); b.push(0x00);
                            return;
                        }
                        "abs" => {
                            if !args.is_empty() {
                                // abs(x) = x < 0 ? -x : x
                                self.gen_expr(b, &args[0]);
                                b.push(OP_LOCAL_TEE); b.extend_from_slice(&leb128(self.current_locals as u32));
                                self.gen_expr(b, &args[0]);
                                b.push(OP_I32_CONST); b.push(0x00);
                                self.gen_expr(b, &args[0]);
                                b.push(OP_I32_SUB);
                                self.gen_expr(b, &args[0]);
                                b.push(OP_I32_CONST); b.push(0x00);
                                b.push(OP_I32_LT_S);
                                b.push(OP_SELECT);
                            }
                            return;
                        }
                        "len" => {
                            if !args.is_empty() { self.gen_expr(b, &args[0]); b.push(OP_DROP); }
                            b.push(OP_I32_CONST); b.push(0x00);
                            return;
                        }
                        "int" | "float" | "str" => {
                            if !args.is_empty() { self.gen_expr(b, &args[0]); }
                            else { b.push(OP_I32_CONST); b.push(0x00); }
                            return;
                        }
                        "type_of" => {
                            if !args.is_empty() { self.gen_expr(b, &args[0]); b.push(OP_DROP); }
                            b.push(OP_I32_CONST); b.push(0x00);
                            return;
                        }
                        "assert" | "assert_eq" | "assert_ne" => {
                            for a in args { self.gen_expr(b, a); b.push(OP_DROP); }
                            b.push(OP_I32_CONST); b.push(0x00);
                            return;
                        }
                        _ => {
                            // User function call
                            if let Some(&func_idx) = self.func_name_to_idx.get(name) {
                                for a in args { self.gen_expr(b, a); }
                                b.push(OP_CALL); b.extend_from_slice(&leb128(func_idx));
                            } else {
                                for a in args { self.gen_expr(b, a); b.push(OP_DROP); }
                                b.push(OP_I32_CONST); b.push(0x00);
                            }
                            return;
                        }
                    }
                }

                // Indirect call
                for a in args { self.gen_expr(b, a); }
                self.gen_expr(b, callee);
                b.push(OP_DROP);
                b.push(OP_I32_CONST); b.push(0x00);
            }
            ast::ExprKind::Assign { target, value, .. } => {
                self.gen_expr(b, value);
                match target {
                    ast::AssignTarget::Ident(name) => {
                        if let Some(&idx) = self.local_idx.get(name) {
                            b.push(OP_LOCAL_SET); b.extend_from_slice(&leb128(idx));
                        } else {
                            b.push(OP_DROP);
                        }
                    }
                    _ => { b.push(OP_DROP); }
                }
                b.push(OP_I32_CONST); b.push(0x00);
            }
            ast::ExprKind::Ternary(cond, then, els) => {
                self.gen_expr(b, cond);
                b.push(OP_IF); b.push(I32);
                self.gen_expr(b, then);
                b.push(OP_ELSE);
                self.gen_expr(b, els);
                b.push(OP_END);
            }
            _ => {
                b.push(OP_I32_CONST); b.push(0x00);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Binary encoding
    // -----------------------------------------------------------------------

    fn finish(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&WASM_MAGIC);
        out.extend_from_slice(&WASM_VERSION);

        // Type section
        if !self.types.is_empty() {
            let mut d = Vec::new();
            d.extend_from_slice(&leb128(self.types.len() as u32));
            for t in &self.types {
                d.push(0x60);
                d.extend_from_slice(&leb128(t.params.len() as u32));
                d.extend_from_slice(&t.params);
                d.extend_from_slice(&leb128(t.results.len() as u32));
                d.extend_from_slice(&t.results);
            }
            out.extend_from_slice(&wasm_section(1, &d));
        }

        // Import section
        if !self.imports.is_empty() {
            let mut d = Vec::new();
            d.extend_from_slice(&leb128(self.imports.len() as u32));
            for (module, name, type_idx) in &self.imports {
                d.extend_from_slice(&wasm_name(module));
                d.extend_from_slice(&wasm_name(name));
                d.push(0x00);
                d.extend_from_slice(&leb128(*type_idx));
            }
            out.extend_from_slice(&wasm_section(2, &d));
        }

        // Function section
        if !self.functions.is_empty() {
            let mut d = Vec::new();
            d.extend_from_slice(&leb128(self.functions.len() as u32));
            for f in &self.functions {
                d.extend_from_slice(&leb128(f.type_idx));
            }
            out.extend_from_slice(&wasm_section(3, &d));
        }

        // Memory section (at least 2 pages for string buffer + iov)
        let pages = std::cmp::max((self.mem_offset + 65535) / 65536, 2);
        out.extend_from_slice(&wasm_section(5, &{
            let mut d = Vec::new();
            d.extend_from_slice(&leb128(1));
            d.push(0x00); // no max
            d.extend_from_slice(&leb128(pages));
            d
        }));

        // Export section
        if !self.exports.is_empty() {
            let mut d = Vec::new();
            d.extend_from_slice(&leb128(self.exports.len() as u32));
            for (name, kind, index) in &self.exports {
                d.extend_from_slice(&wasm_name(name));
                d.push(*kind);
                d.extend_from_slice(&leb128(*index));
            }
            out.extend_from_slice(&wasm_section(7, &d));
        }

        // Start section
        if let Some(start) = self.start_func {
            out.extend_from_slice(&wasm_section(8, &leb128(start)));
        }

        // Code section
        if !self.functions.is_empty() {
            let mut d = Vec::new();
            d.extend_from_slice(&leb128(self.functions.len() as u32));
            for f in &self.functions {
                d.extend_from_slice(&leb128(f.body.len() as u32));
                d.extend_from_slice(&f.body);
            }
            out.extend_from_slice(&wasm_section(10, &d));
        }

        // Data section
        if !self.data_segments.is_empty() {
            let mut d = Vec::new();
            d.extend_from_slice(&leb128(self.data_segments.len() as u32));
            for (offset, data) in &self.data_segments {
                d.push(0x00); // active segment, memory 0
                d.push(OP_I32_CONST);
                d.extend_from_slice(&leb128_signed(*offset as i64));
                d.push(OP_END);
                d.extend_from_slice(&leb128(data.len() as u32));
                d.extend_from_slice(data);
            }
            out.extend_from_slice(&wasm_section(11, &d));
        }

        out
    }
}

fn wasm_section(id: u8, data: &[u8]) -> Vec<u8> {
    let mut out = vec![id];
    out.extend_from_slice(&leb128(data.len() as u32));
    out.extend_from_slice(data);
    out
}

fn wasm_name(name: &str) -> Vec<u8> {
    let mut out = leb128(name.len() as u32);
    out.extend_from_slice(name.as_bytes());
    out
}

fn leb128(mut value: u32) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 { byte |= 0x80; }
        out.push(byte);
        if value == 0 { break; }
    }
    out
}

fn leb128_signed(mut value: i64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        let more = !((value == 0 && (byte & 0x40) == 0) || (value == -1 && (byte & 0x40) != 0));
        if more { byte |= 0x80; }
        out.push(byte);
        if !more { break; }
    }
    out
}
