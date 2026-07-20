# Plix Standard Library (v0.2)

Everything that ships in the language: core builtins (always available) plus
the modules `fs`, `sys`, `net`, `py`, `ai`, `forge` — use `import "fs";`
etc. (see also [ffi-python.md](ffi-python.md) for the Python bridge).

## Constants

`PI` · `E` · `TAU` · `INF` · `NAN`

## I/O

| function | description |
|---|---|
| `say(a, b, ...)` | print values space-separated + newline |
| `print(v)` | print without newline |
| `input([prompt])` | read a line from stdin (no trailing newline) |

## Conversion

| function | description |
|---|---|
| `str(v)` | display form (same as interpolation) |
| `repr(v)` | debug form (strings quoted) |
| `int(v)` | to int (float truncates, `"42"` parses, `true`→1) |
| `float(v)` | to float |
| `bool(v)` | truthiness as `true`/`false` |
| `type_of(v)` | `"int" "float" "bool" "null" "string" "array" "object" "function" "builtin" "pyobject"` |

## Numbers

`abs` `floor` `ceil` `round(x [,ndigits])` `sqrt` `pow(x,y)` `exp` `log`
`sin` `cos` `tan` `atan2(y,x)` `min(a,b)` `max(a,b)` `clamp(x,lo,hi)`
`sign(x)` `idiv(a,b)` (integer division; `/` is always float)
`rand()` (0..1) `rand_int(lo,hi)` (inclusive)

## Strings

| function | description |
|---|---|
| `len(s)` | length in chars |
| `upper(s)` `lower(s)` `trim(s)` | case/whitespace |
| `split(s, sep)` | → array (`""` splits into chars) |
| `join(arr, sep)` | array of strings → one string |
| `replace(s, from, to)` | all occurrences |
| `contains(s, sub)` `starts_with` `ends_with` | → bool |
| `find(s, sub)` | first index or `-1` |
| `chars(s)` | → array of 1-char strings |
| `substr(s, start, len)` | substring (negative start = from end) |
| `char(code)` | unicode codepoint → 1-char string |
| `byte(s)` | first unicode scalar value as int |
| `parse_int(s)` `parse_float(s)` | strict parse (error on junk) |

## Arrays

| function | description |
|---|---|
| `len(a)` | also on strings/objects |
| `push(a, v)` | append in place → `a` |
| `pop(a)` | → last element (error if empty) |
| `insert(a, i, v)` | insert at index |
| `remove_at(a, i)` | remove + return element |
| `index_of(a, v)` | position or `-1` |
| `reverse(a)` | new reversed array |
| `sort(a)` | ascending (numbers/strings) |
| `sort_by(a, cmp)` | `cmp(x,y)` negative/zero/positive |
| `map(a, f)` `filter(a, f)` `each(a, f)` | higher-order |
| `reduce(a, f, init)` | fold left |
| `range(n)` `range(a,b)` `range(a,b,step)` | → array of ints |

## Objects (maps)

| function | description |
|---|---|
| `keys(o)` | sorted keys |
| `values(o)` | values matching `keys(o)` order |
| `entries(o)` | `[[k,v], ...]` sorted by key |
| `has(o, k)` | key exists |
| `get(o, k [,default])` | lookup (default `null`) |
| `set(o, k, v)` | assign (also `o.k = v`) |
| `delete(o, k)` | remove key |

Member syntax sugar: `o.name` ≡ `o["name"]`; assignment `o.name = v`.

## System & misc

| function | description |
|---|---|
| `time_ms()` | unix epoch milliseconds (int) |
| `clock()` | seconds (float, sub-ms resolution) for benchmarks |
| `sleep_ms(ms)` | block the thread |
| `assert(cond [,msg])` | runtime error when falsy (`assert failed: expected truthy, got …`) |
| `assert_eq(actual, expected)` | deep-equality assertion with both values in the message |
| `assert_ne(a, b)` | fails when both sides are deep-equal |
| `panic(msg)` | raise a runtime error |
| `exit(code)` | exit the process |

**`plix test`** runs test suites: files named `*_test.px`; every top-level
`func test_*` executes as one test. Assertions above are the failure
mechanism. See `tests/core_test.px`.

## `fs` — files & paths

| function | description |
|---|---|
| `fs.read(path)` | whole file → string |
| `fs.write(path, s)` | truncate + write |
| `fs.append(path, s)` | append |
| `fs.exists(p)` `fs.is_file(p)` `fs.is_dir(p)` | predicates |
| `fs.size(p)` | bytes |
| `fs.list(dir)` | entry names |
| `fs.mkdir(dir)` | create (recursive) |
| `fs.remove(p)` | delete file/empty dir |
| `fs.rename(a, b)` | move |
| `fs.copy(a, b)` | copy file |
| `fs.join(a, b)` | path join |
| `fs.abs(p)` | absolute path |
| `fs.parent(p)` `fs.name(p)` `fs.ext(p)` | path parts |

## `sys` — process & OS

| function | description |
|---|---|
| `sys.platform()` | `"linux" "macos" "windows" ...` |
| `sys.arch()` | `"x86_64" "aarch64" ...` |
| `sys.args()` | `[program, ...args]` |
| `sys.env(name)` | env var or `null` |
| `sys.set_env(k, v)` | set env var |
| `sys.cwd()` | working directory |
| `sys.exit(code)` | exit |
| `sys.exec(cmd)` | shell string or `["prog", a1, ...]` → `{code, stdout, stderr}` |
| `sys.pid()` | process id |
| `sys.hostname()` | host name |

## `net` — HTTP server & client (http://)

```plix
import "net";
func app(req) {                      // req: {method, path, target, version,
    return net.response(200, "hi");  //        query{}, headers{}, body}
}                                    // return: string | net.response map
net.serve("127.0.0.1:8080", app);    // sequential HTTP/1.1 server
```

| function | description |
|---|---|
| `net.serve(addr, handler)` | bind + loop; handler per request |
| `net.response(code, body [,content_type])` | response map |
| `net.get(url)` | → `{code, body, headers}` |
| `net.post(url, body [,content_type])` | → `{code, body, headers}` |

## `py` / `ai` — Python bridge

See [ffi-python.md](ffi-python.md). Summary:
`py.available` `py.has_module` `py.import` `py.eval` `py.exec` `py.call`
`py.getattr` `py.setattr` `py.hasattr` `py.repr` `py.to_plix`,
`ai.lib` `ai.eval` `ai.array` `ai.call` `ai.shape`.
(`py` is a keyword — `import "py" as python;` to use it as a value.)

## `forge` — Rust bridge

| function | description |
|---|---|
| `forge.version()` | plix version string |
| `forge.rust_version()` | `rustc --version` (or build-time fallback) |
| `forge.cargo(args...)` | run cargo → `{code, stdout, stderr}` |
| `forge.target()` | `{os, arch, family}` of the host |
