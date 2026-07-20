# Plix (`.px`)

A fast, easy, safe programming language with **two hearts**:

- 🏃 **Interpreter** — instant start, great errors: `plix run app.px`
- 🦾 **Native compiler** — real standalone executables via Cranelift:
  `plix build app.px -o app && ./app`

One language, one runtime, identical semantics in both modes.

```plix
func fib(n: int) -> int {               // v0.3: gradual typing
    if (n <= 1) { return n; }
    return fib(n - 1) + fib(n - 2);
}

struct Vec2 { x: float, y: float, name: str = "v" }   // v0.3: OOP
impl Vec2 {
    func norm2(&self) -> float { return self.x * self.x + self.y * self.y; }
}

auto start = clock();
say("fib(30) = ${fib(30)}; |v|² = ${Vec2 { x: 3.0, y: 4.0 }.norm2()}");
say("elapsed: ${round(clock() - start, 3)}s");
```

**fib(30): interpreter ≈ 13.4 s → native dynamic ≈ 1.87 s → native typed ≈ 0.94 s
(≈ 14× faster), byte-identical output in both modes.**

## Why Plix

- **Fast like Rust where it counts.** Whole values are tag-packed words;
  int arithmetic runs inline in generated machine code; the rest calls a
  small shared runtime. Optimized via Cranelift (`opt_level=speed`).
- **Gradually typed (v0.3).** Annotate where it matters — `n: int`,
  `-> float`, struct fields, loop headers. Provable mismatches are compile
  errors (14+ E-codes); dynamic values entering typed slots hit a runtime
  boundary guard. **Provably-typed locals compile unboxed** (raw
  i64/f64/bool + inlined overflow checks): ~2× over dynamic native.
  See [docs/typing.md](docs/typing.md).
- **OOP, Rust's way (v0.3).** `struct` for data, `impl` for behavior,
  `trait` for interfaces — `&self` / `&mut self` receivers, associated
  functions, default trait methods, static trait bounds. **No data
  inheritance**, by design. See [docs/oop.md](docs/oop.md).
- **Easy like Python.** `auto`/`const`, arrays, objects, `match`,
  string interpolation `"${x}"`, closures, for-in, slices — no ceremony.
- **Safe by keywords, not by GC** (see [docs/memory.md](docs/memory.md)):
  - `auto`/`const` → ARC refcount + frame arenas (no pauses, no cycles in practice);
  - `own` → Rust-style **ownership & borrowing**, fully *static*
    (`&x`, `&mut x`, use-after-move `E0382`, …) with **zero runtime cost**.
- **Python inside, at C speed** (see [docs/ffi-python.md](docs/ffi-python.md)):
  numpy/torch/pandas objects stay *Python-side* as handles; calls go straight
  to the CPython C-API. `import py "numpy" as np; np.matmul(a, b)` costs one C call.
- **Real ecosystem**: complete stdlib out of the box — core, strings,
  arrays/objects, math, `fs`, `sys`, `net` (HTTP server+client), `py`/`ai`,
  `forge` (Rust bridge). See [docs/stdlib.md](docs/stdlib.md).

## Install

**One self-contained binary per OS** — Linux x86-64, Windows 10/11 x64,
macOS Intel and Apple Silicon. See the [Releases](../../releases) page for
prebuilt archives, or build from source (just a Rust toolchain):

```bash
cargo build              # debug toolchain  → target/debug/plix
cargo build --release    # fast toolchain  → target/release/plix
bash tests/run_all.sh    # full dual-mode verification battery
```

Details per OS (linker needs for `plix build`, optional libpython
discovery, Windows notes): [docs/install.md](docs/install.md).

## Usage

```
plix run    file.px [args...]   interpret (fast startup)
plix build  file.px -o app      native standalone executable
plix exec   file.px [args...]   compile+run natively (like `go run`)
plix check  file.px             parse + ownership checking only
plix test   [paths...]          run *_test.px suites (default: ./tests)
plix repl                       interactive shell
```

## The language in 30 seconds

```plix
import "fs";

const PI2 = 2 * PI;
own secret = "moves are explicit";   // ownership: `own y = x`, `&x`, `&mut x`

func greet(name, punct = "!") {
    return "hello ${name}${punct}";
}

auto scores = { "ana": 91, "ben": 73 };
for (who in scores) {                // objects iterate sorted keys
    match idiv(scores[who], 10) {
        10 | 9 => say("${who}: A"),
        8 | 7  => say("${who}: B"),
        _      => say("${who}: C")
    }
}

auto evens = filter(range(10), func(x) { return x % 2 == 0; });
say(greet("plix", "!!"), evens, PI2);
```

Full grammar: [docs/grammar.md](docs/grammar.md). Everything is in:
closures, pattern matching (statement + expression), default/rest params,
anonymous functions, ternary, compound assignment, bit ops, modules
(`import "lib.px" as lib`), error traces, **type annotations**,
**struct/impl/trait**, ownership.

## Examples

| file | shows |
|---|---|
| `examples/fib.px` | recursion; benchmark both modes |
| `examples/typed.px` | **v0.3**: gradual typing + specialization |
| `examples/oop.px` | **v0.3**: structs, impl, traits, bound methods |
| `examples/type_err.px` | **v0.3**: the 10 type-checker errors (`plix check`) |
| `examples/features.px` | the whole language core in one file |
| `examples/closures_match.px` | closures, captures, match stmt/expr |
| `examples/ownership_ok.px` | `own` done right |
| `examples/ownership_err.px` | borrow-checker errors (`plix check`) |
| `examples/fs_sys_forge.px` | fs / sys / forge stdlib |
| `examples/module_app.px` + `mathlib.px` | user `.px` modules |
| `examples/python_ai.px` | Python FFI + numpy |
| `examples/http_server.px` | `net.serve` web server |

```bash
plix run examples/typed.px
plix exec examples/oop.px
plix check examples/type_err.px
plix run examples/http_server.px &  curl http://127.0.0.1:8080/
plix build examples/fib.px -o fib && ./fib
```

Every example + guard test runs through **both** backends and must print
byte-identical output; the battery lives in `tests/run_all.sh`. Plus:
`plix test` — a built-in test runner (`*_test.px` files, `func test_*` per
test, `assert*` builtins), and `tests/fuzz_parity.sh` — a deterministic
random-program fuzzer demanding interpreter ≡ native on every seed
(currently **150/150 identical**).

## Project layout

```
src/        toolchain: lexer, parser, typechecker, interpreter, ownership
            checker, resolver, Cranelift codegen, CLI — all in Rust
rt/         plixrt: tagged values, ARC+arenas, ops, structs/instances,
            stdlib, HTTP, Python FFI
            (linked into the toolchain AND into every compiled program)
examples/   runnable demos
docs/       grammar.md · typing.md · oop.md · memory.md · ffi-python.md · stdlib.md
            comparison.md (measured benchmarks vs C/Rust/Java/Node/Python)
```

## خلاصه — فارسی

**پلیکس** یک زبان برنامه‌نویسی کامل و مستقل است که به‌صورت Rust پیاده شده و دو حالت اجرا دارد:

- **مفسر** (`plix run`) برای شروع فوری و خطاهای خوانا
- **کامپایلر native** (`plix build`) با **Cranelift** که فایل اجرایی مستقل و واقعی می‌سازد

ویژگی‌ها:

- **تایپینَک تدریجی (v0.3)**: annotation اختیاری (`n: int`، `-> float`) با خطای کامپایل‌تایم سخت؛ متغیرهای اثبات‌شده در native به‌صورت unboxed (مقدار خام i64/f64) کامپایل می‌شوند — fib(30) از ~۱۳٫۴ ثانیه به **~۰٫۹۴ ثانیه** (~۱۴ برابر) می‌رسد. مستندات: `docs/typing.md`.
- **OOP به سبک Rust (v0.3)**: `struct` + `impl` + `trait` با receiverهای `&self` / `&mut self`، متدهای default و bound method، **بدون وراثت داده**. مستندات: `docs/oop.md`.
- **مدیریت حافظه با کلیدواژه**: `auto`/`const` با شمارش مرجع (ARC) و arena، و `own` با مالکیت و قرض‌گیری استاتیک به سبک Rust (`&x` و `&mut x`) با هزینهٔ اجرایِ صفر.
- **پایتون داخل زبان** با FFI مستقیم روی C-API: کتابخانه‌های numpy/pandas سمت پایتون می‌مانند (بدون کپی — سریع)، و `import py "numpy" as np` انگار کتابخانهٔ خودِ زبان است. ماژول `ai` هم رابط آمادهٔ هوش مصنوعی است.
- **اکوسیستم کامل**: کتابخانهٔ استاندارد شامل core/رشته/آرایه/map/ریاضی، `fs`، `sys`، `net` (سرور و کلاینت HTTP)، `py`/`ai` و `forge` (پل به اکوسیستم Rust).
- گرامر کامل در `docs/grammar.md`؛ مدل حافظه در `docs/memory.md`؛ FFI در `docs/ffi-python.md`؛ مرجع کتابخانه در `docs/stdlib.md`.

هر برنامهٔ پلیکس در هر دو حالت (مفسر و native) **دقیقاً همان خروجی** را می‌دهد — حتی پیام‌های خطا — تست‌شده روی تمام مثال‌های `examples/`.
