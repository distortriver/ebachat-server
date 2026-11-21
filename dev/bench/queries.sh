#!/bin/bash
# What each server asks of Postgres for the same load: pg_stat_statements over one steady step per server.
# queries.sh [impls] [cores] [rps:pps] [seconds]
set -euo pipefail
. "$(dirname "$0")/env.sh"
impls=${1:-"rust go"}
cores=${2:-4}
step=${3:-800:2}
secs=${4:-60}
PGSS=1 "$BENCH/db.sh" restart
"$BENCH/db.sh" analyze
out=$RUN/results/queries-$(date +%m%d-%H%M).txt
psql() { docker exec bench-db psql -U mmuser -d mattermost "$@"; }
for impl in $impls; do
  "$BENCH/server.sh" "$impl" "$cores" | tee -a "$out"
  sleep 5
  psql -qAtc 'SELECT pg_stat_statements_reset()' >/dev/null
  (cd "$REPO" && taskset -c "$LG_CPUS" ./target/release/loadgen steps http://localhost:8065 "$(cat "$RUN/server.pid")" "$step" "$secs") 2>&1 | tee -a "$out"
  {
    echo "== $impl: Postgres totals (websocket connects, 10 s warm-up and the step)"
    psql -c "SELECT sum(calls) AS calls, round(sum(total_exec_time)) AS exec_ms, round(sum(total_exec_time) / sum(calls)::numeric, 3) AS ms_per_call,
                    sum(rows) AS rows, sum(shared_blks_hit + shared_blks_read) AS blocks FROM pg_stat_statements"
    echo "== $impl: top 40 by total time"
    psql -c "SELECT calls, round(total_exec_time) AS total_ms, round(mean_exec_time::numeric, 3) AS mean_ms, rows,
                    left(regexp_replace(query, '\s+', ' ', 'g'), 230) AS query
             FROM pg_stat_statements ORDER BY total_exec_time DESC LIMIT 40"
    echo "== $impl: top 25 by calls"
    psql -c "SELECT calls, round(total_exec_time) AS total_ms, rows, left(regexp_replace(query, '\s+', ' ', 'g'), 230) AS query
             FROM pg_stat_statements ORDER BY calls DESC LIMIT 25"
    echo
  } | tee -a "$out"
done
"$BENCH/server.sh" stop
echo "results: $out"
