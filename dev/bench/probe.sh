#!/bin/bash
# One steady step against one server with the Rust cache counters (Rust only) and pg_stat_statements around it.
# probe.sh <impl> <cores> <rps:pps> <seconds>
set -euo pipefail
. "$(dirname "$0")/env.sh"
impl=$1 cores=$2 step=$3 secs=$4
"$BENCH/server.sh" "$impl" "$cores"
T=$(curl -si -X POST localhost:8065/api/v4/users/login -d '{"login_id":"admin","password":"Admin-pass-123"}' | tr -d '\r' | awk -F': ' 'tolower($1)=="token"{print $2}')
stats() {
  [ "$impl" = rust ] || return 0
  curl -s -H "Authorization: Bearer $T" localhost:8065/api/v4/ebachat/caches \
    | python3 -c 'import json,sys; d=json.load(sys.stdin); [print(k, v) for k, v in d.items() if v.get("misses") or v.get("invalidations")]'
}
psql() { docker exec bench-db psql -U mmuser -d mattermost "$@"; }
pgss=$(psql -qAtc "SELECT count(*) FROM pg_extension WHERE extname = 'pg_stat_statements'")
stats
[ "$pgss" = 1 ] && psql -qAtc 'SELECT pg_stat_statements_reset()' >/dev/null
(cd "$REPO" && taskset -c "$LG_CPUS" ./target/release/loadgen steps http://localhost:8065 "$(cat "$RUN/server.pid")" "$step" "$secs") 2>&1
echo "== caches after"
stats
if [ "$pgss" = 1 ]; then
  psql -c "SELECT sum(calls) AS calls, round(sum(total_exec_time)::numeric) AS exec_ms FROM pg_stat_statements"
  psql -c "SELECT calls, round(total_exec_time::numeric) AS total_ms, round(mean_exec_time::numeric, 3) AS mean_ms, rows,
                  left(regexp_replace(query, '\s+', ' ', 'g'), 150) AS query
           FROM pg_stat_statements ORDER BY total_exec_time DESC LIMIT 12"
fi
"$BENCH/server.sh" stop
