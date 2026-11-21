#!/bin/bash
# Scenario 1, cores: each server on 1, 2, 4, ... cores under the same rising load; every user online,
# 2 posts/s into bench-public all along. scale.sh [impls] [cores] [steps] [seconds per step]
set -euo pipefail
. "$(dirname "$0")/env.sh"
impls=${1:-"rust go"}
cores=${2:-$(c=1; while [ $c -le "$SRV_MAX" ]; do printf "%s " $c; c=$((c * 2)); done)}
steps=${3:-"100:2,200:2,400:2,800:2,1600:2,3200:2"}
secs=${4:-20}
"$BENCH/db.sh" analyze
out=$RUN/results/scale-$(date +%m%d-%H%M).txt
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
