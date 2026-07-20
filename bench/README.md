# bench/ — cross-language benchmark suite

Reproduces the numbers in [docs/comparison.md](../docs/comparison.md).

Programs, identical logic in each language (outputs verified equal):

| Program | Workload |
|---|---|
| `fib.*` | recursive `fib(30)` — call overhead + int arith |
| `loop.*` | 10M-iteration `s = (s + i) % 1000003` — scalar int throughput |
| `strcat.*` | 200k `s = s + "x"` — allocation + immutability cost |
| `array.*` | 1M `push` + sum — collection growth + iteration |
| `hello.*` | startup latency |

Plix variants: `*_typed.px` (typed slots, native `plix build`), `*_dyn.px`
(dynamic, native + interpreter).

## Run

```bash
cd bench
# system compilers
gcc -O2 fib.c -o fib_c && gcc -O2 loop.c -o loop_c && gcc -O2 hello.c -o hello_c
rustc -O fib.rs -o fib_rs && rustc -O loop.rs -o loop_rs
javac Fib.java Loop.java Hello.java
# plix natives
P=../target/release/plix
$P build fib_typed.px -o fib_pt && $P build fib_dyn.px -o fib_pd
$P build loop_typed.px -o loop_pt && $P build loop_dyn.px -o loop_pd
$P build strcat.px -o strcat_pd && $P build array.px -o array_pd
$P build hello.px -o hello_p
# measure (best of 3, wall clock)
python3 run_bench.py
bash mem_check.sh   # peak RSS (needs /usr/bin/time)
```

`run_bench.py` uses `subprocess` + `perf_counter`, best of 3 runs.
Machine used for the published numbers: 2-core Xeon @ 2.60 GHz, 2 GB RAM,
Debian x86-64; GCC 14, rustc 1.97.1, OpenJDK 11, Node 20, CPython 3.13,
Plix 0.3.0.
