# Plix 0.3.0 vs. the big languages

An honest, measured comparison of Plix against C (GCC 14), Rust (1.97),
Java (OpenJDK 11), JavaScript (Node 20), and Python (3.13). Every number
below was produced on this machine; the reproduction scripts live in
[`bench/`](../bench/).

> **Methodology.** Wall-clock, cold process start, best of 3 runs, identical
> program logic and identical output in every language (outputs were verified
> to match exactly). Machine: 2-core Xeon @ 2.60 GHz, 2 GB RAM, Debian
> x86-64. Microbenchmarks measure tiny things precisely; they say nothing
> about large real-world programs. Where a language's result includes a JIT
> or VM warm-up, the startup benchmark below shows that fixed cost.

## Executive summary

| Dimension | Plix's position today | Honest verdict |
|---|---|---|
| Raw compute speed (native, typed) | ~2–4× slower than CPython 3.13 on calls/strings; ~1.3× **faster** than CPython on int loops | mid-pack, behind all JITs and AOT compilers |
| Raw compute (interpreter) | 4–14× slower than CPython | slowest tier by design trade-off (parity, simplicity) |
| Startup latency | 2 ms — ties C, **6× faster than Python, 15× faster than Node, 35× faster than Java** | top tier |
| Memory footprint | ties Rust, **4× leaner than Python, 8× leaner than Node** | top tier |
| Distribution | single static 5.7 MB binary, no runtime needed | top tier (Go/Rust class) |
| Type safety | strong by script-language standards; **compile errors, not warnings**; not Rust-grade | good for its class, soundness story incomplete in dynamic-only code |
| Language features | struct/impl/trait, gradual typing, `own`+borrow checker, Python FFI, fs/sys/net | missing generics depth, exhaustiveness, async/threads, macros, package manager |
| Tooling | run/build/test/fuzz ship today; fmt deferred; no LSP/debugger yet | behind every major language |
| Ecosystem & maturity | v0.3.0, no library ecosystem, no production users | infant stage |

**Bottom line:** as a performance/distribution story Plix already out-classes
CPython on startup and memory and lands within ~2× of it on arithmetic
throughput — but V8 (Node), the JVM, and every AOT compiler beat it by 5–50×
on compute. Its genuine differentiators today are *delivery* (static binary,
instant start, low RSS), *safety defaults* (typed slots are hard errors, the
`own` borrow checker, interp≡native byte parity), and *Python FFI*. On feature
breadth and ecosystem it is where Go was in 2009 or Rust in 2014: promising
core, everything else not built yet.

---

## 1. Benchmarks (measured)

### 1.1 fib(30) — recursive call overhead + integer arithmetic

| Language / mode | Time | vs fastest | vs Plix native-typed |
|---|---|---|---|
| C (gcc -O2) | 0.003 s | 1.0× | — |
| Rust (rustc -O) | 0.004 s | 1.3× | — |
| JavaScript (Node 20, V8 JIT) | 0.041 s | 13× | 4.8× faster |
| Java (OpenJDK 11, HotSpot) | 0.093 s | 31× | 2.1× faster |
| **Python 3.13** | **0.123 s** | **41×** | **1.6× faster** |
| **Plix — native typed** | **0.196 s** | **65×** | **—** |
| Plix — native dynamic | 0.443 s | 148× | 2.3× slower |
| Plix — interpreter | 1.679 s | 560× | 8.6× slower |

Java's number includes ~70 ms of JVM start-up; its steady-state code is
roughly V8-class. The important rows: **CPython 3.13 beats Plix
native-typed on this workload.** Plix's typed-call ABI still boxes arguments
at function boundaries and the Cranelift backend does no inlining — call-heavy
code is exactly where that hurts (see §3 for the plan).

### 1.2 Tight loop, 10M iterations — scalar integer throughput

| Language / mode | Time | vs fastest | vs Python 3.13 |
|---|---|---|---|
| C (gcc -O2) | 0.042 s | 1.0× | 38× faster |
| Rust (rustc -O) | 0.044 s | 1.05× | 36× faster |
| JavaScript (Node 20) | 0.073 s | 1.7× | 22× faster |
| Java (OpenJDK 11) | 0.099 s | 2.4× | 16× faster |
| **Plix — native typed** | **1.270 s** | **30×** | **1.26× faster than Python** |
| **Python 3.13** | **1.605 s** | **38×** | — |
| Plix — native dynamic | 2.449 s | 58× | 1.5× slower |
| Plix — interpreter | 5.698 s | 136× | 3.6× slower |

On pure typed integer work, Plix-native is **faster than CPython** — the
typed-slot machinery (62-bit integers, hard range guards) pays off. But
V8's JIT is 17× faster still, and C is 30× faster: there is one full
performance tier missing (inlining, range-hoisting, rt fast paths).

### 1.3 String concatenation ×200k — allocation + immutability

| Language / mode | Time | vs fastest |
|---|---|---|
| JavaScript (Node 20) | 0.044 s | 1.0× |
| Python 3.13 | 0.862 s | 20× |
| Plix — native dynamic | 1.640 s | 37× |
| Plix — interpreter | 1.735 s | 39× |

V8's rope/`ConsString` wins outright. CPython's refcounted `s += "x"` fast
path beats Plix ~2×: Plix allocates a fresh heap string per iteration and the
arena delays reuse of the intermediate buffers. A builder/owned-string fast
path is future work, not a fundamental limit (same O(n) class as Python).

### 1.4 Array push + sum ×1M — collection growth + iteration

| Language / mode | Time | vs fastest | Peak RSS |
|---|---|---|---|
| Rust (rustc -O) | 0.014 s | 1.0× | 9.9 MB |
| JavaScript (Node 20) | 0.085 s | 6× | 88.3 MB |
| Python 3.13 | 0.231 s | 17× | 48.0 MB |
| Plix — native dynamic | 0.446 s | 32× | **10.7 MB** |
| Plix — interpreter | 0.948 s | 68× | 11.8 MB |

Two stories here. Speed: Plix is ~2× behind CPython's list. **Memory: Plix
ties Rust and is 4.5× leaner than CPython and 8× leaner than Node** — the ARC
+ arena model frees aggressively and boxes far less per element than CPython.

### 1.5 Startup — hello world

| Language / mode | Time |
|---|---|
| C | 0.001 s |
| **Plix — native** / interpreter | **0.002 s** |
| Python 3.13 | 0.013 s |
| JavaScript (Node 20) | 0.031 s |
| Java (OpenJDK 11) | 0.071 s |

Plix binaries start like C binaries: **6× faster than Python, 15× faster than
Node, 35× faster than Java.** For CLI tools and serverless cold-start this
matters more than loop throughput.

### 1.6 Distribution

| | Single-file binary | Runtime required | Binary size |
|---|---|---|---|
| Plix | yes | **none** | 5.7 MB (hello world, fully static) |
| Rust | yes | none | 4.4 MB |
| Go | yes | none | ~2 MB |
| C | yes | libc | ~20 KB |
| Python / Node | no (without packagers) | interpreter + stdlib | — |
| Java | no (without jlink) | JRE | — |

Plix ships in the same delivery class as Go and Rust, which Python and JS
still don't have a first-party answer for.

---

## 2. Language-level comparison

| | **Plix 0.3** | Python 3.13 | TypeScript/Node | Go 1.x | Rust | Java |
|---|---|---|---|---|---|---|
| Typing | gradual, **hard errors** for typed slots | gradual, optional, lint-only | gradual, erased at run time | static | static + ownership | static |
| Type violations | compile error | run-time `TypeError` | none at run time (if any) | compile error | compile error | compile error |
| `null` safety | `null` exists; Option not yet | `None` unchecked | sound only in strict mode | nil pointers | `Option<T>` forced | null everywhere |
| Memory mgmt | ARC + arena, plus `own` borrow checker | refcount + cycle GC | tracing GC | tracing GC | ownership (compile-time) | tracing GC |
| Concurrency | **none yet** | GIL threads + asyncio + 3.13 free-threaded | event loop | goroutines | threads + async | threads + virtual threads |
| OOP | struct/impl/trait, **no data inheritance** | class + multiple inheritance | class + structural interfaces | composition only | trait objects | class + interface |
| Generics | arrays/maps erased; **monomorphization not yet** | n/a (runtime) | full, erased | yes | full | type erasure |
| FFI / interop | **built-in Python bridge (`py`) + Rust bridge (`forge`)** | C API / ctypes | N-API / npm native | cgo (slow) | excellent C ABI | JNI / Panama |
| Stdlib breadth | fs / sys / net / ai | enormous | enormous (npm) | large | moderate | enormous |
| Tooling | run · build · test · fuzz · (fmt pending) · no LSP yet | rich | extremely rich | excellent first-party | excellent first-party | extremely rich |
| Build one artifact | yes, static | no | bundlers | yes | yes | jlink/jar |
| Package manager | **none yet** | pip | npm | modules+proxy | cargo | maven/gradle |
| Production maturity | v0.x pre-release | massive | massive | massive | large | massive |

### Where Plix genuinely leads (not ties)

1. **Interp ≡ native byte parity** — none of the compared languages ship a
   pair of backends contractually required to produce byte-identical output
   including error text and exit codes (verified by a 150-seed fuzzer).
   JS has ~5 engines with observable differences; Python has none to compare.
2. **Strictness-by-default ergonomics**: type violations are *errors before
   run*, not IDE hints (unlike Python linters/TS-any holes), at script-like
   syntax cost.
3. **Python FFI as a builtin** (`py.` / `ai.`) with memory-managed handles —
   none of the other five embed a foreign-ecosystem bridge *in the runtime*.
4. **Static binary + 2 ms startup + ~10 MB RSS** in one package; Go comes
   close, but Go has no interpreter mode for instant edit-run cycles.

### Where Plix is clearly behind (the honest list)

1. **Compute speed vs every JIT and every AOT compiler** — 5× (V8) to 50× (C)
   on numeric code; call-heavy code even loses to CPython 3.13 today.
2. **No concurrency model at all** — every compared language has at least
   one first-class answer.
3. **No package manager / library ecosystem** — language value in 2026 is
   mostly ecosystem value; this is the deepest hole.
4. **No LSP / debugger / IDE story** — a hard gate for serious adoption.
5. **Generics are parsed-but-erased**, no exhaustiveness checking, no
   `Option` yet — type-system depth trails TS, Rust, Java, Go.
6. **Battle-testing**: no production users, no security model audit, no
   stable-ABI commitment (v0.x semantics may break).

---

## 3. What would move the numbers (performance roadmap, by expected gain)

| Change | Target workload | Expected effect |
|---|---|---|
| Typed-call fast ABI (no boxing at call sites) + Cranelift inlining | fib-style call-heavy | 2–4× on calls → passes CPython overall |
| rt fast paths (inline `mk_int`/tag ops in CLIF instead of extern calls) | loops, arrays | 1.5–3× general |
| String builder / owned-concat op (`s +=` reuses capacity) | strcat class | 2–5× strings → passes CPython |
| Range-hoist the 62-bit guards out of loops | loops | 1.2–1.5× loops |
| Direct rt array ops for `push`/index hot paths | arrays | 1.5–2× collections |
| A optimizing e-graph / regalloc quality (Cranelift is by design ~2× behind LLVM -O2) | everything | closes part of the gap to C/Rust tier; the rest needs llvm alt or JIT |

None of these change the language — they raise the floor of `plix build`
output. The feature roadmap v0.4+ (generics depth, exhaustiveness, `Option`,
formatter, LSP, package manager, concurrency) follows the phase plan agreed
for the project: Phase 1 tooling (test/fuzz — done), Phase 2 type-system
depth, Phase 3 compiler optimization.

## خلاصه — فارسی

مقایسهٔ واقعی و اندازه‌گیری‌شدهٔ Plix 0.3 با C، Rust، Java، Node و Python
(اسکریپت‌های بازتولید در `bench/`):

- **سرعت محاسبات خالص:** نسخهٔ native با تایپ‌گذاری روی حلقه‌های عددی ~۱٫۳
  برابر از پایتون ۳٫۱۳ *سریع‌تر* است، ولی روی کد با فراخوانی تابع زیاد
  (fib) ~۱٫۶ برابر *کندتر* از پایتون است — چون آرگومان‌ها مرز تابع‌ها را
  باکس‌شده عبور می‌کنند و inlining نداریم. Node و Java و C و Rust همه از
  Plix جلوترند (۵ تا ۵۰ برابر روی اعداد).
- **حافظه:** Plix با Rust تساوی می‌کند؛ ۴٫۵ برابر کمتر از پایتون و ۸ برابر
  کمتر از Node (مدل ARC + arena واقعاً جواب می‌دهد).
- **راه‌اندازی:** ۲ میلی‌ثانیه — مثل C؛ ۶ برابر سریع‌تر از پایتون و ۱۵ برابر
  سریع‌تر از Node. برای ابزارهای خط فرمان (CLI) عالی است.
- **تحویل:** باینری تک‌فایل ایستا بدون نیاز به runtime — در کلاس Go/Rust.
- **برتری‌های واقعی:** برابری بایت-به-بایت interpreter و native، درج خطای
  تایپ به‌صورت compile error، پل داخلی به Python (`py`/`ai`)،
  static binary + startup سریع + مصرف RAM پایین در یک پکیج.
- **عقب‌ماندگی‌های صادقانه:** بدون concurrency، بدون package manager و
  اکوسیستم، بدون LSP، generics ناقص (فاز ۲)، خروجی کد کندتر از همهٔ
  JIT/AOTها.

جمع‌بندی: Plix امروز از نظر *تحویل و وزن و ایمنیِ پیش‌فرض* بالاتر از کلاس
زبان‌های اسکریپتی است، و از نظر سرعت محاسبات خام در همسایگی CPython قرار
دارد؛ فاصلهٔ اصلی با «زبان‌های بزرگ» اکوسیستم و یک طبقهٔ بهینه‌سازی کامپایلر
است، نه طراحی هسته.
