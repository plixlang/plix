# Python FFI — libraries that feel native, at C speed

Plix talks to CPython **directly through the C API**, discovered at runtime
with `dlopen`/`dlsym` — no pyo3, no Python headers at build time, no link
flags. `plix`-built executables stay self-contained; if a `libpython3.x.so`
is on the machine, they use it.

```plix
import "py" as python;        // the py module (keyword `py`, so alias it)
import py "numpy" as np;      // import a python module like a native one

auto m = np.array([[1, 2, 3], [4, 5, 6]]);
say(m.sum());                 // 21   ← numpy does the work, plix gets a number
say(m.mean());                // 3.5
say(m.shape);                 // (2, 3)
say(np.dot([1, 2, 3], [4, 5, 6]));   // 32
```

## The speed model ("not slow like python")

Crossing Plix ↔ Python blindly (converting everything, every call) is what
makes bridges slow. Plix avoids that:

1. **Heavy objects never cross.** numpy arrays, torch modules, pandas
   frames stay *on the Python side* as opaque handles (`pyobject`). A call
   like `np.matmul(a, b)` costs exactly one C-API call; the megabytes in
   `a`/`b` are never copied.
2. **Attributes are resolved once.** `m.sum` is captured in a `PyBound`
   value — repeated `m.sum()` calls skip the getattr round-trip.
3. **Only cheap values convert.** int / float / bool / string / list /
   tuple / dict / None cross eagerly. numpy *scalars* (`np.int64`…) convert
   automatically too, via an `item()` fast path, so `arr.sum()` returns a
   plain Plix `3.5`, not a handle.
4. When you *want* a big object converted, say so explicitly:
   `python.to_plix(t)` (uses `tolist()`/`item()` behind the scenes).

## Reference

| function | meaning |
|---|---|
| `import py "mod" as x` | import a python module (runtime action, usable inside `if`) |
| `python.available()` | is a libpython present? |
| `python.has_module("numpy")` | can the module be imported? (never raises) |
| `python.eval("2 ** 10")` | evaluate an expression |
| `python.exec("x = 5")` | run statements |
| `python.call(f, ...)` | call a python-side callable |
| `python.getattr(o, "n")` / `python.setattr(o, "n", v)` / `python.hasattr` | attributes |
| `python.repr(o)` | python `repr` as a Plix string |
| `python.to_plix(o)` | deep conversion (lists, dicts, numpy via `tolist`) |

Member access on a handle works like a real object: `m.sum()`, `m.shape`,
`np.arange(6).reshape(2, 3)`.

## `ai.*` — the Plix-branded AI face

Same bridge, batteries included:

| function | meaning |
|---|---|
| `ai.lib("torch")` | import (alias of `import py`) |
| `ai.eval(src)` | python expression |
| `ai.array([1.5, 2.5])` | numpy array (handle, zero copy) |
| `ai.call(obj, "method", a, b)` | method call by name |
| `ai.shape(v)` | numpy shape (deep-converted) or `[len(v)]` for arrays |

## Configuration

The bridge probes, in order: `$PLIX_PYTHON_LIB`, then
`libpython3.13..3.8(.so.1.0)` on the default linker path, `/usr/local/lib`,
`/usr/lib`, `/usr/lib/x86_64-linux-gnu`. The GIL is held per operation;
types with no cheap conversion never leave Python land, so nothing copies
unless you ask.

```bash
# if autodetect fails:
export PLIX_PYTHON_LIB=/usr/local/lib/libpython3.13.so
```
