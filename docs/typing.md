# Gradual Typing in Plix (v0.3)

Plix is a **gradually typed** language: every value carries its type, and you
add *static* types exactly where they pay off — hot loops, public APIs,
struct fields. Everything without an annotation behaves dynamically,
unchanged from v0.2. Where you *do* annotate, the checker is **strict**:
mismatches are compile errors (`plix check`, and both `run` and `build`),
not warnings.

Both backends (interpreter and native compiler) enforce the identical
semantics. A program produces the same output — including error messages —
in `plix run` and in its compiled executable.

## Where annotations go

```plix
func fib(n: int) -> int {              // parameter + return type
    if (n <= 1) { return n; }
    return fib(n - 1) + fib(n - 2);
}

auto total: int = 0;                   // variable
for (auto i: int = 1; i <= n; i += 1)  // C-style loop header
    { total += i; }
for (v: int in xs) { ... }             // for-in element
struct Vec2 { x: float, y: float, name: str = "v" }   // fields
```

Type names: `any int float bool str array map func Result` and any user
struct/trait name. `array<T>` and `map<K, V>` are checked at typed boundaries
such as variable declarations, index assignment, for-in element annotations and
struct fields. Nullable values are written as `T?` or `Option<T>`; `None` is
an alias for `null`, and `Some(x)` is the non-null constructor. Function values
can be typed with `(A, B) -> R`, for example `auto f: (int, int) -> int = add;`.
Recoverable APIs can use `Result<T, E>` with `Ok(v)` / `Err(e)` and exhaustive
`match`. Nullary user enums are supported for finite-state values:
`enum Color { Red, Green, Blue }`.

## The contract

1. **Provably wrong = compile error.** Passing a `str` literal to an
   `int` parameter is `E0308` before the program ever runs.

2. **Unknown = trusted, then verified.** Passing a *dynamic* value (a
   function returning an unannotated type, an untyped array element, …) into
   a typed slot is allowed — and checked at runtime by a **boundary guard**:

   ```plix
   func id(x) { return x; }        // untyped: returns Any
   auto n: int = id("hi");         // compiles fine…
   // RuntimeError: type guard: expected int for n, found non-int
   ```

   Guards fire at exactly the same points in both backends: typed parameter
   binding, annotated variable declarations, typed for-in elements,
   assignments/compound assignments to typed locals, and declared return
   types.

3. **Conversions are explicit-ish.** An `int` may flow into a `float` slot
   (widening — `1` becomes `1.0` and is *represented* as a float). Nothing
   else converts implicitly. A `bool` slot stores the *truthiness* of the
   assigned value as a real `true`/`false`, exactly like the native bool
   slot does.

## Typed `int` arithmetic is strict

Dynamic ints are 62-bit and *promote* to floats on overflow (Python-ish).
Typed ints are **machine ints with checked arithmetic** — overflow is a
`RuntimeError`, in both backends:

```plix
func big() -> int {
    auto x: int = 4611686018427387903;   // 2^62 - 1, the largest int
    x += x;                              // RuntimeError: integer overflow
                                         //   in typed int addition
    return x;
}
```

Division/remainder by zero and negative shift counts are errors everywhere
(typed or dynamic): `division by zero`, `remainder by zero`,
`negative shift count`.

> Note: the interpreter enforces the same strictness through checker
> annotations on the AST, so `run` and the compiled binary stay
> byte-identical.

## Specialization (why annotations are fast)

When a local's type is *provably* `int`/`float`/`bool` for its whole
lifetime (annotated or inferred — e.g. `auto i = 0` in a counter loop), the
native backend stores it **unboxed**: a raw i64/f64/bool register instead of
a tagged heap word. Arithmetic on it compiles to single machine
instructions with inlined overflow checks; guards only exist where a
dynamic value crosses in.

```plix
func fib(n: int) -> int {        // every op inside runs unboxed
    if (n <= 1) { return n; }
    return fib(n - 1) + fib(n - 2);
}
```

fib(30) on this machine: interpreter ≈ 13.4 s, native dynamic ≈ 1.87 s,
**native typed ≈ 0.94 s**.

Differences you may observe (all deliberate):

- A *raw* local read before its first store yields `0` / `0.0` / `false`,
  not `null`. (Reading any variable before assignment is a logic bug; the
  dynamic path gives `null`.)
- Speculatively: nothing else. If you find another divergence, it is a bug.

## Errors reference (the new E-codes)

| code  | meaning |
|-------|---------|
| E0308 | type mismatch (argument, variable, return value, index, …) |
| E0061 | arity mismatch |
| E0599 | unknown method/member |
| E0609 | field does not exist |
| E0063 | missing struct field in literal |
| E0625 | field initializer type mismatch |
| E0618 | value of non-function type called |
| E0277 | trait bound not satisfied / cannot index/iterate |
| E0594 | `&mut self` method called on a `const` |
| E0124 | missing `return` in a function with a declared return type |
| E0412 | unknown type name |
| E0428 | duplicate definition |
| E0053 | method signature does not match the trait declaration |
| E0046 | missing trait item in `impl Trait for S` |
| E0407 | method not a member of the trait |
| E0119 | conflicting implementations |

With `plix check` all of the above are recoverable-style diagnostics: every
error in the file is reported in one pass (see `examples/type_err.px`).

## What is *not* typed (deliberately)

- Container elements: `array<int>` is parsed but erased. Full generics are
  a later milestone.
- Function values: first-class functions stay dynamically checked at call
  time (arity/type errors surface at the call boundary).
- `null`: there is no `Option` yet; `null` flows untyped.
