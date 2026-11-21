#!/bin/bash
# Scenario 2, one channel everybody is in: every user online, a steady 100 req/s of the usual mix, and a rising
# rate of posts into bench-public, each delivered to every websocket. fanout.sh [impls] [cores] [steps] [seconds]
set -euo pipefail
. "$(dirname "$0")/env.sh"
impls=${1:-"rust go"}
cores=${2:-$SRV_MAX}
steps=${3:-"100:1,100:5,100:10,100:20,100:50,100:100,100:200"}
secs=${4:-20}
"$BENCH/db.sh" analyze
out=$RUN/results/fanout-$(date +%m%d-%H%M).txt
for impl in $impls; do
  for c in $cores; do
    "$BENCH/server.sh" "$impl" "$c" | tee -a "$out"
    sleep 5
    (cd "$REPO" && taskset -c "$LG_CPUS" ./target/release/loadgen steps http://localhost:8065 "$(cat "$RUN/server.pid")" "$steps" "$secs") 2>&1 | tee -a "$out"
    echo | tee -a "$out"
  done
done
"$BENCH/server.sh" stop
echo "results: $out"
