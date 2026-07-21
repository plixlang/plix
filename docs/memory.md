# Plix Memory Model

Three declaration keywords, two strategies — you pick per variable.

| keyword | strategy | cost | checked |
|---------|----------|------|---------|
| `auto`  | ARC refcount + per-frame arena | tiny (no GC pauses) | — |
| `const` | like `auto` + immortal binding | tiny | E0594 on rebinding |
| `own`   | ownership & borrowing, like Rust | **zero** (static) | E0373/E0382/E0499/E0502/E0503/E0506/E0594 |

## `auto` / `const` — automatic

Every heap value has a reference count. Results of expressions live in the
current **frame arena** and are freed in bulk:

- after each expression statement (arena rewind),
- after every loop iteration,
- when a function's frame pops (its whole arena dies).

Storing a value into a variable **retains** it (+1); overwriting or leaving
scope **releases** it. This gives deterministic cleanup with no tracing GC
and no stop-the-world pause. Scalars (`int`, `float`, `bool`, `null`)
never touch the heap.

## `own` — ownership, statically checked (no runtime cost)

`own` behaves like Rust ownership, adapted to Plix's flavour:

```
own name = "plix";
say(name);            // borrows (calls never move)
own award = name;     // EXPLICIT MOVE — `name` is now unusable
// say(name);         // error[E0382]: use of moved value
```

Rules enforced by `plix run`, `plix check`, and `plix build` alike:

1. **Moves are always explicit**: `own y = x`, `return x`, or consuming a
   container with `for (x in xs)`. Use-after-move → `E0382`.
2. **Function arguments borrow, never move.** Parameters behave like
   `auto` bindings inside the callee — so passing an `own` value to
   `say(x)`, `len(x)`, or your own functions never consumes it.
3. **`&x`** immutable borrow (many at once), **`&mut x`** mutable borrow
   (exclusive). Borrows end with the enclosing statement, so an in-flight
   borrow can't outlive its statement: moving while borrowed → `E0503`,
   two conflicting borrows → `E0499`/`E0502`, writing while borrowed →
   `E0506`.
4. **Closures capture by reference through heap cells.** An `own` value
   cannot be captured (`E0373`) — copy it into an `auto` first.
5. **Copy types**: `int`, `float`, `bool`, `null` are never "moved" —
   reads copy them (just like Rust's `Copy`).
6. You may not move the same value in a loop without reinitializing it
   (`E0382` loop form), and `const` bindings may not be reassigned
   (`E0594`).

Errors look like this:

```
error[E0382]: use of moved value "a"
  --> examples/ownership_err.px:6:5
   |
  6| say(a);             // ERROR E0382: use of moved value `a`
   |     ^
   = note: "a" was moved here (at 5:9)
```

## What the compiler generates

Cranelift emits straight-line int arithmetic inline (values are tag-tagged
words; ±/comparisons on ints are a few machine instructions with a range
check). Everything else calls the shared runtime — the same code the
interpreter uses, so semantics are *identical* in both modes. `plix build`
links a real `main()` + `libplixrt.a` into a standalone ELF executable.

### Specialization

Locals whose type is *provably* `int`/`float`/`bool` for their entire
lifetime — annotated (`auto i: int`) or inferred (`auto i = 0`) — are
stored **unboxed**: raw i64/f64/bool machine values, no tag, no refcount,
no heap. Arithmetic on them is native instructions with inlined overflow
checks; the only boxing/unboxing cost lives at *typed boundaries* (a
dynamic value entering a typed slot), where a runtime guard verifies the
type in the interpreter and the compiled binary alike. Raw slots never
escape: boxed views are materialized on demand for captures/calls.

## Struct instances

An instance (`p = Point { x: 1.0, y: 2.0 }`) is a single heap object:
the struct descriptor + one flat vector of field values. It follows the
exact same rules as an array under each keyword:

- `auto p = Point { ... }` — refcounted like any heap value; fields are
  retain/released recursively; `p.x = v` retains `v` and releases the old
  field value.
- `own p = Point { ... }` — owned; moves are explicit (`own q = p` moves),
  method calls borrow the receiver (`&self` / `&mut self`, matching the
  declared receiver), and the borrow checker applies as usual.
- Bound methods (`auto f = p.dist`) are one tiny heap pair {receiver, fn}.



## Reality notes / current limits

- ARC is **not** cycle-collecting (self-referential structures
  leak; acyclic data is fully reclaimed).
- The `own` checker treats whole containers as one unit (no per-element
  borrow splitting like Rust's).
- In native code, reading a local before its declaration line yields
  `null` (flat namespace) instead of an error — the interpreter still
  errors. Declare before use.
- Specialized (unboxed) locals read as `0` / `0.0` / `false` before their
  first store instead of `null`. Same rule: declare before use — with an
  annotation the declaration has an initializer anyway.
