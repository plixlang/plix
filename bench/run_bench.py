#!/usr/bin/env python3
"""Benchmark harness: run each candidate N times, report min wall time."""
import subprocess, sys, time

RUNS = 3

def bench(name, cmd):
    best = None
    out = None
    for _ in range(RUNS):
        t0 = time.perf_counter()
        r = subprocess.run(cmd, capture_output=True, text=True)
        dt = time.perf_counter() - t0
        if out is None:
            out = (r.stdout + r.stderr).strip().splitlines()
            out = out[-1] if out else ""
        best = dt if best is None else min(best, dt)
    print(f"{name:28s} {best:9.3f}s   [{out}]")
    return name, best

PLIX = "/home/user/plix/target/release/plix"

cases = {
    "fib(30) recursive": [
        ("C (gcc -O2)",            ["./fib_c"]),
        ("Java (JDK 11, JIT)",     ["java", "-cp", ".", "Fib"]),
        ("JavaScript (Node 20)",   ["node", "fib.js"]),
        ("Python 3.13",            ["python3", "fib.py"]),
        ("Plix — native typed",    ["./fib_pt"]),
        ("Plix — native dynamic",  ["./fib_pd"]),
        ("Plix — interpreter",     [PLIX, "run", "fib_dyn.px"]),
    ],
    "loop 10M (int arith)": [
        ("C (gcc -O2)",            ["./loop_c"]),
        ("Java (JDK 11, JIT)",     ["java", "-cp", ".", "Loop"]),
        ("JavaScript (Node 20)",   ["node", "loop.js"]),
        ("Python 3.13",            ["python3", "loop.py"]),
        ("Plix — native typed",    ["./loop_pt"]),
        ("Plix — native dynamic",  ["./loop_pd"]),
        ("Plix — interpreter",     [PLIX, "run", "loop_dyn.px"]),
    ],
    "strcat 200k": [
        ("JavaScript (Node 20)",   ["node", "strcat.js"]),
        ("Python 3.13",            ["python3", "strcat.py"]),
        ("Plix — native dynamic",  ["./strcat_pd"]),
        ("Plix — interpreter",     [PLIX, "run", "strcat.px"]),
    ],
    "array push/sum 1M": [
        ("JavaScript (Node 20)",   ["node", "array.js"]),
        ("Python 3.13",            ["python3", "array.py"]),
        ("Plix — native dynamic",  ["./array_pd"]),
        ("Plix — interpreter",     [PLIX, "run", "array.px"]),
    ],
    "startup: hello world": [
        ("C (gcc -O2)",            ["./hello_c"]),
        ("JavaScript (Node 20)",   ["node", "hello.js"]),
        ("Python 3.13",            ["python3", "hello.py"]),
        ("Java (JDK 11, JIT)",     ["java", "-cp", ".", "Hello"]),
        ("Plix — native",          ["./hello_p"]),
        ("Plix — interpreter",     [PLIX, "run", "hello.px"]),
    ],
}

title = sys.argv[1] if len(sys.argv) > 1 else "all"
for group, entries in cases.items():
    print(f"\n--- {group} ---")
    results = [bench(n, c) for n, c in entries]
    fastest = min(t for _, t in results)
    for n, t in results:
        print(f"    -> {n:26s} {t/fastest:6.2f}x of fastest")
