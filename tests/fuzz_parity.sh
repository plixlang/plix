#!/usr/bin/env bash
# Fuzz parity: generate N deterministic random programs and demand identical
# behavior from the interpreter and the native backend (output AND exit
# code, modulo source-location prefixes).
#
# usage: tests/fuzz_parity.sh [N] [path-to-plix]
set -u
cd "$(dirname "$0")/.."
N="${1:-40}"
PLIX="${2:-./target/debug/plix}"
WORK=target/fuzz
mkdir -p "$WORK"
norm() { grep -v -E '^(trace|  in )' | sed 's/^[^ ]*\.px:[0-9]*:[0-9]*: //; s/ (at [0-9:]*)//'; }

fail=0
for seed in $(seq 1 "$N"); do
  f="$WORK/fz_$seed.px"
  $PLIX run tests/fuzz_gen.px "$seed" > "$f" 2>/dev/null
  i=$($PLIX run "$f" 2>&1 | norm); ic=$?
  if ! $PLIX build "$f" -o "$WORK/fz_$seed" >/dev/null 2>&1; then
    echo "SEED $seed: native compile FAILED (interp output:"; echo "$i" | head -3; echo ")"
    cp "$f" "$WORK/fail_$seed.px"
    fail=$((fail+1))
    continue
  fi
  n=$("$WORK/fz_$seed" 2>&1 | norm); nc=$?
  if [ "$i" != "$n" ] || [ "$ic" != "$nc" ]; then
    echo "SEED $seed: PARITY FAIL (interp=$ic native=$nc)"
    diff <(printf '%s\n' "$i") <(printf '%s\n' "$n") | head -8
    cp "$f" "$WORK/fail_$seed.px"
    fail=$((fail+1))
  else
    rm -f "$f" "$WORK/fz_$seed"
  fi
done
echo "fuzz parity: $((N-fail))/$N seeds identical ($fail failed)"
[ "$fail" -eq 0 ]
