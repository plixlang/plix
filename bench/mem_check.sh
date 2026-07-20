#!/bin/bash
# Peak RSS (KB) of key programs on the array benchmark (1M-element workload).
cd /home/user/bench
PLIX=/home/user/plix/target/release/plix
run() {
    local name="$1"; shift
    local kb
    kb=$(/usr/bin/time -f %M "$@" 2>&1 >/dev/null | tail -1)
    printf "%-26s %8s KB peak RSS\n" "$name" "$kb"
}
echo "--- peak memory, array push/sum 1M ---"
run "JavaScript (Node 20)" node array.js
run "Python 3.13" python3 array.py
run "Plix - native dynamic" ./array_pd
run "Plix - interpreter" "$PLIX" run array.px
echo "--- binary/runtime footprint ---"
ls -la fib_pt hello_p 2>/dev/null | awk '{printf "%-26s %10d bytes\n", $NF, $5}'
