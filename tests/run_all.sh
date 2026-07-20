#!/usr/bin/env bash
# Plix dual-mode verification battery.
#
# For every example and guard test: run through the interpreter AND the
# compiled native binary, then require byte-identical output (timing lines
# and source-location prefixes excluded). Also runs the negative checker
# suites (typecheck + ownership).
#
# usage:  tests/run_all.sh [path-to-plix]
set -u
cd "$(dirname "$0")/.."
PLIX="${1:-./target/debug/plix}"
BIN="${PLIX_BIN_DIR:-$HOME/examples_bin}"
mkdir -p "$BIN"
norm() { grep -v -E '^(trace|  in )' | sed 's/^[^ ]*\.px:[0-9]*:[0-9]*: //; s/ (at [0-9:]*)//' | grep -v -E 'elapsed|pid|argv'; }

pass=0; fail=0
check() { # name, interp_out, native_out
  if [ "$2" == "$3" ] && [ -n "$2" ]; then pass=$((pass+1));
  else fail=$((fail+1)); echo "FAIL $1"; diff <(printf '%s' "$2") <(printf '%s' "$3") | head -5; fi
}

echo "== examples (dual mode, byte-identical) =="
for ex in fib features closures_match fs_sys_forge module_app ownership_ok oop typed; do
  i=$($PLIX run "examples/$ex.px" 2>&1 | norm)
  $PLIX build "examples/$ex.px" -o "$BIN/$ex" >/dev/null 2>&1
  n=$("$BIN/$ex" 2>&1 | norm)
  check "$ex" "$i" "$n"
done

echo "== guard parity =="
shopt -s nullglob
for t in tests/guards/g*.px; do
  g=$(basename "$t" .px)
  i=$($PLIX run "$t" 2>&1 | norm)
  $PLIX build "$t" -o "$BIN/$g" >/dev/null 2>&1
  n=$("$BIN/$g" 2>&1 | norm)
  check "$g" "$i" "$n"
done

echo "== negative suites =="
te=$($PLIX check examples/type_err.px 2>&1 | grep -c E0)
oe=$($PLIX check examples/ownership_err.px 2>&1 | grep -c E03)
[ "$te" -eq 10 ] && [ "$oe" -ge 2 ] && pass=$((pass+1)) || { fail=$((fail+1)); echo "FAIL checkers (type=$te own=$oe)"; }

echo
echo "=== $pass passed, $fail failed ==="
[ "$fail" -eq 0 ]
