# Plix Language Grammar (v0.3)

Complete EBNF reference. Terminals are in `"quotes"`. Plix source is UTF-8;
identifiers are `[A-Za-z_][A-Za-z0-9_]*`. New in v0.3: type annotations,
`struct` / `impl` / `trait`, struct literals.

```
program        := { top_item }
top_item       := import_stmt | func_decl | struct_decl | enum_decl | impl_block
               |  trait_decl | var_decl | statement

import_stmt    := "import" string ["as" ident] ";"            (* user .px or native stdlib *)
               |  "import" "py" string ["as" ident] ";"       (* python module *)

var_decl       := ("auto" | "const" | "own") ident [":" type] "=" expr ";"

type           := ident ["<" type {"," type} ">"]        (* int float bool str array
                   map func | user struct | user trait; generic args parsed, erased *)

func_decl      := "func" ident "(" [params] ")" ["->" type] block
params         := param {"," param} [","]
param          := receiver | ["..."] ident [":" type] ["=" expr]
                      (* `...name` collects the rest, once, last *)
receiver       := "&" "self" | "&mut" "self" | "self"   (* methods only;
                   bare self is sugar for &self *)

struct_decl    := "struct" ident "{" field {"," field} [","] "}"
field          := ident [":" type] ["=" expr]

enum_decl      := "enum" ident ["<" ident {"," ident} ">"]
                  "{" enum_variant {"," enum_variant} [","] "}"
enum_variant   := ident ["(" type {"," type} ")"]
                  (* nullary variants are fully supported; payload sums are
                     represented today by built-in Option/Result constructors *)

impl_block     := "impl" ident ["for" ident] "{" {func_decl} "}"
                      (* impl S              → inherent items
                         impl Trait for S    → trait implementation *)
trait_decl     := "trait" ident "{" {trait_item} "}"
trait_item     := "func" ident "(" receiver {"," param} ")" ["->" type]
                  (";" | block)         (* `;` required, block = default *)

block          := "{" {statement} "}"

statement      := var_decl
               |  func_decl
               |  import_stmt
               |  expr_stmt
               |  block
               |  if_stmt
               |  while_stmt
               |  for_stmt
               |  match_stmt
               |  return_stmt
               |  "break" ";"
               |  "continue" ";"

expr_stmt      := expr ";"
if_stmt        := "if" "(" expr ")" block ["else" (if_stmt | block)]
while_stmt     := "while" "(" expr ")" block
for_stmt       := "for" "(" [var_decl | expr ";"] [expr ";"] [expr] ")" block  (* C-style *)
               |  "for" "(" ident [":" type] "in" expr ")" block               (* iteration *)
match_stmt     := "match" expr match_tail   (* statement form *)
match_tail     := "{" match_arm {"," match_arm} [","] "}"
match_arm      := patterns "=>" (expr | block)
patterns       := pattern {"|" pattern}
pattern        := int | float | string | "true" | "false" | "null" | "_" | ident
return_stmt    := "return" [expr] ";"

expr           := assign
assign         := ternary [assign_op assign]                  (* right assoc; target: ident | index | member *)
assign_op      := "=" | "+=" | "-=" | "*=" | "/=" | "%="
ternary        := or_expr ["?" expr ":" ternary]
or_expr        := and_expr {"||" and_expr}
and_expr       := equality {"&&" equality}
equality       := comparison {("==" | "!=") comparison}
comparison     := b_or     {("<" | "<=" | ">" | ">=") b_or}
b_or           := b_xor    {"|" b_xor}
b_xor          := b_and    {"^" b_and}
b_and          := shift    {"&" shift}
shift          := additive {("<<" | ">>") additive}
additive       := multiplicative {("+" | "-") multiplicative}
multiplicative := unary    {("*" | "/" | "%") unary}
unary          := ("!" | "-" | "~") unary | borrow | postfix
borrow         := "&" postfix | "&mut" postfix
postfix        := primary {call_args | index | slice | member}
call_args      := "(" [expr {"," expr}] ")"
index          := "[" expr "]"
slice          := "[" [expr] ".." [expr] "]"
member         := "." ident
primary        := number | float | string | "true" | "false" | "null" | ident
               |  "(" expr ")"
               |  array_lit | object_lit
               |  struct_lit
               |  "func" ident? "(" [params] ")" ["->" type] block   (* anonymous fn *)
               |  match_expr
array_lit      := "[" [expr {"," expr} [","]] "]"
object_lit     := "{" [ident ":" expr {"," ident ":" expr} [","]] "}"
struct_lit     := ident "{" [init {"," init} [","]] "}"     (* only where an
                   expression can start; ambiguity with object_lit is resolved
                   by the declared struct name *)
init           := ident [":" expr]          (* `x` = shorthand for `x: x` *)
match_expr     := "match" expr match_tail    (* all arms must be expr form *)
```

## Lexical rules

- **Comments**: `// line` and `/* block */`.
- **Numbers**: `42`, `0x2A`, `0o52`, `0b101010`, `1_000_000`, floats `3.14`,
  `1e-3`. Integers larger than ±2⁶² become floats automatically (dynamic
  code); in typed code, overflowing `int` arithmetic is a RuntimeError.
- **Strings**: double quoted, escapes `\\ \" \' \n \t \r \0 \xNN \$`;
  interpolation `"result: ${expr}"` (any expression inside `${...}`,
  including nested strings and parentheses);
  raw strings `r"no \escapes ${here}"`.
- **Keywords**: `auto const own func return if else for while break continue
  match true false null import as py in struct impl trait for`.
  (`self` is not a keyword — it's special only in receiver position.)
- **Operators as above**; statement-level expressions need a trailing `;`.
- `if` / `while` / `for` conditions require parentheses.
- `->` is the return-type arrow (`->` and `- >` are lexed distinctly).

## Behavior notes (natural questions)

- `/` is always float division (Python-style): `7 / 2 = 3.5`; use `idiv(a, b)`
  for integer division, `%` for remainder.
- Truthiness: `null`, `false`, `0`, `0.0`, `""`, `[]`, `{}` are falsy;
  everything else is truthy.
- Negative indices: `arr[-1]` is the last element; slices clamp safely:
  `s[1..]`, `arr[..3]`, `arr[0..2]`.
- `==` deep-compares arrays/objects/struct instances structurally.
- `for (x in v)`: arrays iterate by value (live view — mutating `arr[i]`
  is visible), strings iterate characters, objects iterate **sorted keys**.
- `Vec2(1.0, 2.0)` is sugar for `Vec2.new(1.0, 2.0)` when a `new` associated
  function exists; otherwise a struct literal: `Vec2 { x: 1.0, y: 2.0 }`.
- Type annotations (v0.3): anywhere you write `: int`, `: float`, `: bool`,
  `: str`, a struct or trait name, the checker is strict — provable
  mismatches are compile errors; genuinely-dynamic values get a runtime
  boundary guard with an identical message in both backends.
  See [typing.md](typing.md).
- Methods: `p.f(x)` calls as `f(&p, x)`/`f(&mut p, x)` per the receiver;
  bound methods (`auto g = p.f`) are first-class. See [oop.md](oop.md).
- Native mode (`plix build`/`exec`): function bodies are a *flat namespace*
  (no redeclaration of the same name in one body); importing user `.px`
  modules is top-level only in the main file; globals can't shadow builtins.
  `import py "…"` is allowed anywhere (it's a runtime action).
- Struct/impl/trait declarations are **top-level items** (inside a block is
  a resolve error).
